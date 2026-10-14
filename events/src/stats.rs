//! `stats`: a once-per-second summary for when a line per event is unreadable.
//!
//! On a terminal it redraws in the alternate screen (and prints the last frame on exit). When
//! stdout isn't a terminal it prints one plain summary line per second instead.

use std::io::{IsTerminal, Write};

use chrono::Local;
use crossterm::{cursor, execute, queue, terminal};

use crate::app::View;
use crate::lag::{Stage, fmt_ms, fmt_ns};
use crate::metrics::Metrics;
use crate::pipeline::{Change, Pipeline};
use crate::style::{self, ColorChoice, Line, Painter, fmt_count, fmt_rate, println_or_exit};

pub struct StatsOptions {
    pub color: ColorChoice,
    /// Shown in the title, e.g. `dat401-cdc (us-east-1) from latest`.
    pub title: String,
    pub live: bool,
}

pub struct StatsView {
    opts: StatsOptions,
    tty: bool,
    paint: Painter,
    in_alt_screen: bool,
}

fn restore_terminal() {
    let _ = execute!(
        std::io::stdout(),
        cursor::Show,
        terminal::LeaveAlternateScreen
    );
}

impl StatsView {
    pub fn new(opts: StatsOptions) -> StatsView {
        let stdout = std::io::stdout();
        let tty = stdout.is_terminal();
        let paint = Painter::for_stream(opts.color, &stdout);
        let mut v = StatsView {
            opts,
            tty,
            paint,
            in_alt_screen: false,
        };
        if tty
            && execute!(
                std::io::stdout(),
                terminal::EnterAlternateScreen,
                cursor::Hide
            )
            .is_ok()
        {
            v.in_alt_screen = true;
            let prev = std::panic::take_hook();
            std::panic::set_hook(Box::new(move |info| {
                restore_terminal();
                prev(info);
            }));
        }
        v
    }

    fn frame(&self, m: &Metrics, p: &Pipeline, height: usize) -> Vec<Line> {
        let now = m.now_sec();
        let mut lines = Vec::new();
        let mut l = Line::new();
        l.push("Aurora DSQL CDC", style::bold()).plain(format!(
            " · {} · up {}s · {}",
            self.opts.title,
            m.uptime_secs(),
            Local::now().format("%H:%M:%S")
        ));
        if !m.clusters.is_empty() {
            l.plain(" · cluster ").push(
                m.clusters.iter().cloned().collect::<Vec<_>>().join(", "),
                style::table(),
            );
        }
        lines.push(l);

        let mut l = Line::new();
        l.push(
            format!("{:<9}", if self.opts.live { "Kinesis" } else { "Replay" }),
            style::bold(),
        );
        let mut k = vec![format!("{} shards", m.shards.len())];
        if self.opts.live {
            k.push(format!(
                "MillisBehindLatest max {}",
                m.max_behind_ms().map(fmt_ms).unwrap_or_else(|| "—".into())
            ));
            k.push(format!(
                "{} GetRecords/s",
                fmt_rate(m.polls.per_sec(now, 5))
            ));
        }
        k.push(format!("{} records/s", fmt_rate(m.records.per_sec(now, 5))));
        if self.opts.live {
            k.push(format!("throttled {}", fmt_count(m.throttles)));
            k.push(format!("errors {}", fmt_count(m.errors)));
        }
        if m.shards_closed > 0 {
            k.push(format!("{} closed", m.shards_closed));
        }
        l.plain(k.join(" · "));
        lines.push(l);
        lines.push(Line::new());

        // Rates by table and op.
        let tw = m
            .by_table_op
            .keys()
            .map(|(t, _)| t.chars().count())
            .max()
            .unwrap_or(5)
            .clamp(5, 24);
        let avg_n = m.window_secs.min(10);
        let mut h = Line::new();
        h.push(
            format!(
                "{:<tw$}  {:<6}  {:>9}  {:>9}  {:>12}",
                "TABLE",
                "OP",
                "/s",
                format!("avg {avg_n}s"),
                "total"
            ),
            style::bold(),
        );
        lines.push(h);
        // Leave room for the sections below.
        let room = height.saturating_sub(lines.len() + 17).max(3);
        let rows: Vec<_> = m.by_table_op.iter().collect();
        for ((table, op), r) in rows.iter().take(room) {
            let mut l = Line::new();
            l.push(
                format!("{:<tw$}", crate::format::truncate(table, tw)),
                style::table(),
            )
            .plain("  ")
            .push(format!("{:<6}", op.label()), style::op_style(*op))
            .plain(format!(
                "  {:>9}  {:>9}  {:>12}",
                fmt_rate(r.per_sec(now, 1)),
                fmt_rate(r.per_sec(now, avg_n)),
                fmt_count(r.total)
            ));
            lines.push(l);
        }
        if rows.len() > room {
            lines.push(
                Line::new()
                    .push(format!("… {} more", rows.len() - room), style::dim())
                    .clone(),
            );
        }
        if rows.is_empty() {
            lines.push(Line::new().push("(no changes yet)", style::dim()).clone());
        }
        let mut l = Line::new();
        l.push(
            format!(
                "{:<tw$}  {:<6}  {:>9}  {:>9}  {:>12}",
                "all",
                "",
                fmt_rate(m.changes.per_sec(now, 1)),
                fmt_rate(m.changes.per_sec(now, avg_n)),
                fmt_count(m.changes.total)
            ),
            style::bold(),
        );
        lines.push(l);
        lines.push(Line::new());

        // Lag by stage.
        lines.push(
            Line::new()
                .push(
                    format!(
                        "{:<14}  {:>7}  {:>7}  {:>7}  {:>9}",
                        format!("LAG ({}s)", m.window_secs),
                        "p50",
                        "p99",
                        "max",
                        "n"
                    ),
                    style::bold(),
                )
                .clone(),
        );
        for stage in Stage::ALL {
            let mut l = Line::new();
            l.plain(format!("{:<14}  ", stage.label()));
            match m.lag_summary(stage) {
                Some(q) => {
                    for v in [q.p50, q.p99, q.max] {
                        let s = style::lag_style(Some(v));
                        let s = if stage == Stage::Total { s.bold() } else { s };
                        l.push(format!("{:>7}", fmt_ns(v)), s).plain("  ");
                    }
                    l.push(format!("{:>9}", fmt_count(q.count)), style::dim());
                }
                None => {
                    l.push(format!("{:>7}  {:>7}  {:>7}", "—", "—", "—"), style::dim());
                }
            }
            lines.push(l);
        }
        lines.push(Line::new());

        let cs = p.chunk_stats();
        let mut l = Line::new();
        l.push(format!("{:<9}", "Chunked"), style::bold()).plain(format!(
            "{} reassembled from {} fragments · {} waiting · {} fragments buffered · {} gave up · {} crc errors",
            fmt_count(cs.reassembled),
            fmt_count(cs.fragments_used),
            p.chunks_pending(),
            p.fragments_buffered(),
            fmt_count(cs.expired_mains),
            fmt_count(cs.crc_errors)
        ));
        lines.push(l);
        let c = &p.counters;
        let mut l = Line::new();
        l.push(format!("{:<9}", "Other"), style::bold())
            .plain(format!(
                "{} duplicates dropped · {} unparseable · {} unknown type · {} filtered out",
                fmt_count(c.duplicates + cs.duplicate_mains),
                fmt_count(c.parse_errors),
                fmt_count(c.unknown_types),
                fmt_count(c.filtered)
            ));
        lines.push(l);
        if !m.warnings.is_empty() {
            lines.push(Line::new());
            for w in m.warnings.iter().rev().take(3) {
                lines.push(Line::new().push(format!("! {w}"), style::warn()).clone());
            }
        }
        lines
    }

    fn plain_line(&self, m: &Metrics, p: &Pipeline) -> String {
        let now = m.now_sec();
        let mut parts = vec![format!(
            "{} {} ev/s",
            Local::now().format("%H:%M:%S"),
            fmt_rate(m.changes.per_sec(now, 1))
        )];
        let tables: Vec<String> = m
            .by_table_op
            .iter()
            .filter(|(_, r)| r.per_sec(now, 1) > 0.0)
            .map(|((t, op), r)| format!("{t} {} {}", op.label(), fmt_rate(r.per_sec(now, 1))))
            .collect();
        if !tables.is_empty() {
            parts.push(format!("[{}]", tables.join(", ")));
        }
        let lag: Vec<String> = Stage::ALL
            .iter()
            .filter_map(|s| {
                let q = m.lag_summary(*s)?;
                Some(format!(
                    "{} {}/{}/{}",
                    s.label(),
                    fmt_ns(q.p50),
                    fmt_ns(q.p99),
                    fmt_ns(q.max)
                ))
            })
            .collect();
        if !lag.is_empty() {
            parts.push(format!("lag p50/p99/max {}", lag.join(" ")));
        }
        if self.opts.live {
            parts.push(format!(
                "behind {}",
                m.max_behind_ms().map(fmt_ms).unwrap_or_else(|| "—".into())
            ));
        }
        parts.push(format!("{} shards", m.shards.len()));
        for (n, what) in [(m.throttles, "throttled"), (m.errors, "errors")] {
            if n > 0 {
                parts.push(format!("{what} {}", fmt_count(n)));
            }
        }
        let cs = p.chunk_stats();
        parts.push(format!(
            "chunked {} ({} fragments)",
            cs.reassembled, cs.fragments_used
        ));
        if cs.crc_errors > 0 {
            parts.push(format!("crc errors {}", cs.crc_errors));
        }
        parts.join(" · ")
    }

    fn draw(&self, m: &Metrics, p: &Pipeline) {
        let (w, h) = terminal::size()
            .map(|(w, h)| (w as usize, h as usize))
            .unwrap_or((100, 40));
        let mut out = std::io::stdout().lock();
        let _ = queue!(out, cursor::MoveTo(0, 0));
        for line in self.frame(m, p, h).iter().take(h) {
            let _ = write!(
                out,
                "{}",
                line.render(&self.paint, Some(w.saturating_sub(1)))
            );
            let _ = queue!(out, terminal::Clear(terminal::ClearType::UntilNewLine));
            let _ = write!(out, "\r\n");
        }
        let _ = queue!(out, terminal::Clear(terminal::ClearType::FromCursorDown));
        let _ = out.flush();
    }
}

impl View for StatsView {
    fn change(&mut self, _c: &Change, _m: &Metrics) {}

    fn warning(&mut self, msg: &str) {
        // On a terminal warnings show in the frame; otherwise they go to stderr as they happen.
        if !self.tty {
            eprintln!("! {msg}");
        }
    }

    fn tick(&mut self, m: &Metrics, p: &Pipeline) {
        if self.tty {
            self.draw(m, p);
        } else {
            println_or_exit(&self.plain_line(m, p));
        }
    }

    fn finish(&mut self, m: &Metrics, p: &Pipeline) {
        if self.in_alt_screen {
            restore_terminal();
            self.in_alt_screen = false;
            let w = terminal::size().map(|(w, _)| w as usize).ok();
            for line in self.frame(m, p, usize::MAX / 2) {
                println_or_exit(&line.render(&self.paint, w.map(|w| w.saturating_sub(1))));
            }
        } else {
            println_or_exit(&self.plain_line(m, p));
        }
    }
}

impl Drop for StatsView {
    fn drop(&mut self) {
        if self.in_alt_screen {
            restore_terminal();
        }
    }
}
