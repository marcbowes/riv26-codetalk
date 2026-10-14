//! `tail`: one colored line per change, plus a status line.
//!
//! When stdout and stderr are both terminals the status line stays pinned at the bottom
//! (cleared before each burst of output, redrawn after). Otherwise it's printed every ~2 s on
//! stderr, so `--json | jq` and redirects stay clean.

use std::io::{BufWriter, IsTerminal, Stdout, Write};

use chrono::{DateTime, Local};

use crate::app::View;
use crate::format::{TimeCtx, row_summary};
use crate::lag::{Stage, fmt_ms, fmt_ns, fmt_opt};
use crate::metrics::Metrics;
use crate::pipeline::{Change, Pipeline, short_tx};
use crate::record::Op;
use crate::style::{self, ColorChoice, Line, Painter, exit_on_broken_pipe, fmt_count, fmt_rate};

const LAG_W: usize = 7;
const STATUS_EVERY_TICKS: u64 = 2;

pub struct TailOptions {
    pub json: bool,
    pub color: ColorChoice,
    pub quiet: bool,
    pub live: bool,
}

pub struct TailView {
    out: BufWriter<Stdout>,
    paint: Painter,
    epaint: Painter,
    opts: TailOptions,
    sticky: bool,
    status_shown: bool,
    status: Line,
    table_w: usize,
    /// stdout terminal width, refreshed every tick.
    width: Option<usize>,
    ticks: u64,
    time: TimeCtx,
}

impl TailView {
    pub fn new(opts: TailOptions) -> TailView {
        let stdout = std::io::stdout();
        let stderr = std::io::stderr();
        let sticky = stdout.is_terminal() && stderr.is_terminal() && !opts.quiet;
        let mut v = TailView {
            paint: Painter::for_stream(opts.color, &stdout),
            epaint: Painter::for_stream(opts.color, &stderr),
            out: BufWriter::with_capacity(64 * 1024, stdout),
            sticky,
            status_shown: false,
            status: Line::new(),
            table_w: 8,
            width: None,
            ticks: 0,
            time: TimeCtx::local(),
            opts,
        };
        v.width = terminal_width();
        if !v.opts.json {
            v.header();
        }
        v
    }

    fn width(&self) -> Option<usize> {
        self.width
    }

    fn header(&mut self) {
        let legend = "lag per stage: cdc = commit→CDC · kinesis = CDC→Kinesis · here = Kinesis→here · total = commit→here";
        let mut l = Line::new();
        l.plain(format!(
            "{:<12} {:<6} {:<tw$} {:<8} {:>w$} {:>w$} {:>w$} {:>w$}  row",
            "commit",
            "op",
            "table",
            "tx",
            "cdc",
            "kinesis",
            "here",
            "total",
            tw = self.table_w,
            w = LAG_W
        ));
        let width = self.width();
        let legend = Line::new()
            .push(legend, style::dim())
            .render(&self.paint, width);
        let header = Line::new()
            .push(l.render(&Painter::new(false), width), style::bold())
            .render(&self.paint, width);
        self.write_line(&legend);
        self.write_line(&header);
    }

    fn write_line(&mut self, s: &str) {
        self.clear_status();
        if let Err(e) = writeln!(self.out, "{s}") {
            exit_on_broken_pipe(&e);
        }
    }

    fn clear_status(&mut self) {
        if self.status_shown {
            eprint!("\r\x1b[2K");
            self.status_shown = false;
        }
    }

    fn flush(&mut self) {
        if let Err(e) = self.out.flush() {
            exit_on_broken_pipe(&e);
        }
    }

    fn draw_status(&mut self) {
        if self.status.width() == 0 {
            return;
        }
        let width = crossterm::terminal::size()
            .ok()
            .map(|(w, _)| (w as usize).saturating_sub(1));
        let s = self.status.render(&self.epaint, width);
        eprint!("\r\x1b[2K{s}");
        self.status_shown = true;
    }

    fn line(&self, c: &Change) -> Line {
        let mut l = Line::new();
        let time = c
            .commit_display_ns
            .map(|ns| {
                DateTime::from_timestamp_nanos(ns)
                    .with_timezone(&Local)
                    .format("%H:%M:%S%.3f")
                    .to_string()
            })
            .unwrap_or_else(|| "?".into());
        l.plain(format!("{time:<12} "));
        let op = match c.op {
            Op::Other => format!("{}?", c.env.op.as_deref().unwrap_or("").to_uppercase()),
            op => op.label().to_owned(),
        };
        l.push(format!("{op:<6}"), style::op_style(c.op)).plain(" ");
        l.push(
            format!("{:<w$}", c.env.table(), w = self.table_w),
            style::table(),
        )
        .plain(" ");
        l.push(format!("{:<8}", short_tx(c.env.tx_id())), style::dim())
            .plain(" ");
        for stage in [
            Stage::CommitToCdc,
            Stage::CdcToKinesis,
            Stage::KinesisToHere,
        ] {
            let v = c.lag.get(stage);
            l.push(format!("{:>LAG_W$}", fmt_opt(v)), style::lag_style(v))
                .plain(" ");
        }
        let total = c.lag.total;
        l.push(
            format!("{:>LAG_W$}", fmt_opt(total)),
            style::lag_style(total).bold(),
        )
        .plain("  ");
        if c.fragments > 0 {
            l.push(format!("[{} fragments] ", c.fragments), style::dim());
        }
        let (before, after) = (c.env.before_value(), c.env.after_value());
        l.plain(row_summary(
            c.env.table(),
            c.op,
            before.as_ref(),
            after.as_ref(),
            &self.time,
        ));
        l
    }

    fn update_status(&mut self, m: &Metrics, p: &Pipeline) {
        let mut l = Line::new();
        l.push(
            "● ",
            if self.opts.live {
                style::ok()
            } else {
                style::table()
            },
        );
        let mut parts: Vec<String> = Vec::new();
        if !self.opts.live {
            parts.push("replay".into());
        }
        let shards = m.shards.len();
        parts.push(format!(
            "{shards} shard{}",
            if shards == 1 { "" } else { "s" }
        ));
        if self.opts.live {
            parts.push(format!(
                "behind {}",
                m.max_behind_ms().map(fmt_ms).unwrap_or_else(|| "—".into())
            ));
        }
        parts.push(format!(
            "{} ev/s",
            fmt_rate(m.changes.per_sec(m.now_sec(), 5))
        ));
        match m.lag_summary(Stage::Total) {
            Some(q) => parts.push(format!(
                "total p50 {} p99 {} (last {}s, n={})",
                fmt_ns(q.p50),
                fmt_ns(q.p99),
                m.window_secs,
                fmt_count(q.count)
            )),
            None => parts.push(format!("no changes in the last {}s", m.window_secs)),
        }
        let cs = p.chunk_stats();
        if cs.reassembled > 0 {
            parts.push(format!("{} chunked", cs.reassembled));
        }
        if p.chunks_pending() > 0 {
            parts.push(format!("{} chunked waiting", p.chunks_pending()));
        }
        for (n, what) in [
            (cs.crc_errors, "crc errors"),
            (p.counters.duplicates, "dups"),
            (m.throttles, "throttled"),
            (m.errors, "errors"),
        ] {
            if n > 0 {
                parts.push(format!("{} {what}", fmt_count(n)));
            }
        }
        l.push(parts.join(" · "), style::dim());
        self.status = l;
    }
}

fn terminal_width() -> Option<usize> {
    if !std::io::stdout().is_terminal() {
        return None;
    }
    crossterm::terminal::size().ok().map(|(w, _)| w as usize)
}

impl View for TailView {
    fn change(&mut self, c: &Change, _m: &Metrics) {
        if self.opts.json {
            let text = match &c.raw {
                Some(v) => v.to_string(),
                None => return,
            };
            self.write_line(&text);
            return;
        }
        let table_len = c.env.table().chars().count();
        if table_len > self.table_w {
            self.table_w = table_len.min(24);
        }
        let width = self.width().map(|w| w.saturating_sub(1));
        let line = self.line(c).render(&self.paint, width);
        self.write_line(&line);
    }

    fn warning(&mut self, msg: &str) {
        self.flush();
        self.clear_status();
        eprintln!("{}", self.epaint.paint(&format!("! {msg}"), style::warn()));
        if self.sticky {
            self.draw_status();
        }
    }

    fn batch_done(&mut self, _m: &Metrics, _p: &Pipeline) {
        self.flush();
        // Only if output since the last draw cleared it; content refreshes on tick.
        if self.sticky && !self.status_shown {
            self.draw_status();
        }
    }

    fn tick(&mut self, m: &Metrics, p: &Pipeline) {
        self.width = terminal_width();
        self.ticks += 1;
        if self.ticks % STATUS_EVERY_TICKS != 1 {
            return;
        }
        self.update_status(m, p);
        if self.opts.quiet {
            return;
        }
        self.flush();
        if self.sticky {
            self.draw_status();
        } else {
            eprintln!("{}", self.status.render(&self.epaint, None));
        }
    }

    fn finish(&mut self, m: &Metrics, p: &Pipeline) {
        self.flush();
        self.clear_status();
        let mut by_op = [0u64; 4];
        for ((_, op), r) in &m.by_table_op {
            by_op[*op as usize] += r.total;
        }
        let ops: Vec<String> = [Op::Insert, Op::Update, Op::Delete, Op::Other]
            .iter()
            .filter(|op| by_op[**op as usize] > 0)
            .map(|op| format!("{} {}", op.label(), fmt_count(by_op[*op as usize])))
            .collect();
        let mut parts = vec![format!(
            "{} change{} in {}s{}",
            fmt_count(m.changes.total),
            if m.changes.total == 1 { "" } else { "s" },
            m.uptime_secs(),
            if ops.is_empty() {
                String::new()
            } else {
                format!(" ({})", ops.join(", "))
            }
        )];
        if let Some(q) = m.lag_summary(Stage::Total) {
            parts.push(format!(
                "total lag p50 {} p99 {} max {}",
                fmt_ns(q.p50),
                fmt_ns(q.p99),
                fmt_ns(q.max)
            ));
        }
        let cs = p.chunk_stats();
        if cs.reassembled > 0 {
            parts.push(format!(
                "{} chunked ({} fragments)",
                cs.reassembled, cs.fragments_used
            ));
        }
        for (n, what) in [
            (cs.crc_errors, "crc errors"),
            (cs.expired_mains, "chunked gave up"),
            (p.counters.duplicates, "duplicates dropped"),
            (p.counters.parse_errors, "unparseable"),
            (p.counters.filtered, "filtered out"),
            (m.errors, "errors"),
        ] {
            if n > 0 {
                parts.push(format!("{} {what}", fmt_count(n)));
            }
        }
        eprintln!(
            "{}",
            self.epaint
                .paint(&format!("── {}", parts.join(" · ")), style::dim())
        );
    }
}
