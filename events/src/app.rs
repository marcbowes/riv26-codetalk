//! The event loop: source messages in, pipeline, metrics, view out.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use tokio::sync::mpsc;

use crate::lag::now_ns;
use crate::metrics::Metrics;
use crate::pipeline::{Change, Event, Piece, Pipeline};

/// What sources (Kinesis pollers, replay) send to the loop.
#[derive(Debug)]
pub enum SourceMsg {
    /// One GetRecords response (or one replayed line).
    Batch {
        shard: Arc<str>,
        pieces: Vec<Piece>,
        millis_behind: Option<i64>,
    },
    ShardOpened(Arc<str>),
    /// Closed by resharding and fully read.
    ShardClosed(Arc<str>),
    Throttled,
    Error(String),
    /// Replay reached the end of the file.
    End,
}

pub trait View {
    fn change(&mut self, c: &Change, m: &Metrics);
    fn warning(&mut self, msg: &str);
    /// After a burst of messages has been handled.
    fn batch_done(&mut self, _m: &Metrics, _p: &Pipeline) {}
    fn tick(&mut self, m: &Metrics, p: &Pipeline);
    fn finish(&mut self, m: &Metrics, p: &Pipeline);
}

pub struct AppOptions {
    /// Live: stamp "here" on arrival and expire chunks by the wall clock.
    pub live: bool,
    pub tick: Duration,
    pub save: Option<PathBuf>,
}

/// Messages handled before giving the view a chance to redraw.
const BURST: usize = 512;

pub async fn run(
    mut rx: mpsc::Receiver<SourceMsg>,
    mut pipeline: Pipeline,
    view: &mut dyn View,
    mut metrics: Metrics,
    opts: AppOptions,
) -> Result<()> {
    let mut save = match &opts.save {
        Some(p) => Some(BufWriter::new(
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(p)
                .with_context(|| format!("opening {}", p.display()))?,
        )),
        None => None,
    };
    let mut ticker = tokio::time::interval(opts.tick);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let ctrl_c = tokio::signal::ctrl_c();
    tokio::pin!(ctrl_c);

    let mut ctx = Ctx {
        events: Vec::new(),
        last_error_shown: None,
        suppressed_errors: 0,
        live: opts.live,
    };
    let mut done = false;
    while !done {
        tokio::select! {
            biased;
            _ = &mut ctrl_c => break,
            _ = ticker.tick() => {
                pipeline.tick(opts.live.then(now_ns), &mut ctx.events);
                ctx.flush(view, &mut metrics);
                view.tick(&metrics, &pipeline);
                if let Some(w) = save.as_mut() {
                    w.flush().context("writing --save file")?;
                }
            }
            msg = rx.recv() => {
                let Some(msg) = msg else { break };
                done = ctx.handle(msg, &mut pipeline, view, &mut metrics, save.as_mut())?;
                let mut n = 1;
                while !done && n < BURST {
                    match rx.try_recv() {
                        Ok(msg) => done = ctx.handle(msg, &mut pipeline, view, &mut metrics, save.as_mut())?,
                        Err(_) => break,
                    }
                    n += 1;
                }
                view.batch_done(&metrics, &pipeline);
            }
        }
    }

    if !opts.live {
        pipeline.finish(&mut ctx.events);
        ctx.flush(view, &mut metrics);
    }
    if let Some(w) = save.as_mut() {
        w.flush().context("writing --save file")?;
    }
    view.finish(&metrics, &pipeline);
    Ok(())
}

struct Ctx {
    events: Vec<Event>,
    last_error_shown: Option<Instant>,
    suppressed_errors: u64,
    live: bool,
}

impl Ctx {
    /// Returns true when the source is finished.
    fn handle(
        &mut self,
        msg: SourceMsg,
        pipeline: &mut Pipeline,
        view: &mut dyn View,
        metrics: &mut Metrics,
        mut save: Option<&mut BufWriter<File>>,
    ) -> Result<bool> {
        match msg {
            SourceMsg::Batch {
                shard,
                pieces,
                millis_behind,
            } => {
                metrics.record_poll(&shard, millis_behind, pieces.len());
                for mut piece in pieces {
                    let wall = now_ns();
                    if self.live {
                        piece.here_ns = Some(wall);
                    }
                    if let Some(w) = save.as_deref_mut() {
                        let line = crate::replay::save_line(
                            &piece.data,
                            piece.arrival_ns,
                            piece.here_ns,
                            &shard,
                        );
                        writeln!(w, "{line}").context("writing --save file")?;
                    }
                    pipeline.push(piece, wall, &mut self.events);
                    self.flush(view, metrics);
                }
            }
            SourceMsg::ShardOpened(id) => metrics.shard_opened(&id),
            SourceMsg::ShardClosed(id) => metrics.shard_closed(&id),
            SourceMsg::Throttled => metrics.throttles += 1,
            SourceMsg::Error(e) => {
                metrics.errors += 1;
                metrics.last_error = Some(e.clone());
                // Transient errors can repeat on every shard every poll; show at most one per 10 s.
                let quiet = self
                    .last_error_shown
                    .is_some_and(|t| t.elapsed() < Duration::from_secs(10));
                if quiet {
                    self.suppressed_errors += 1;
                } else {
                    let more = match self.suppressed_errors {
                        0 => String::new(),
                        n => format!(" (+{n} more since the last one)"),
                    };
                    self.suppressed_errors = 0;
                    self.last_error_shown = Some(Instant::now());
                    let msg = format!("{e}{more}");
                    metrics.warn(&msg);
                    view.warning(&msg);
                }
            }
            SourceMsg::End => return Ok(true),
        }
        Ok(false)
    }

    fn flush(&mut self, view: &mut dyn View, metrics: &mut Metrics) {
        for ev in self.events.drain(..) {
            match ev {
                Event::Change(c) => {
                    metrics.record_change(&c);
                    view.change(&c, metrics);
                }
                Event::Warning(w) => {
                    metrics.warn(&w);
                    view.warning(&w);
                }
            }
        }
    }
}
