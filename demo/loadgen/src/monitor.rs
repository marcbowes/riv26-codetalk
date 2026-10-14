//! Collects stats four times a second, keeps a one-second sliding window for
//! the live numbers, the sparkline history, cumulative totals and a
//! per-second timeline for the report.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::runner::{Ctx, Mode};
use crate::stats::{Class, Interval, NCLASS, pct_ms};
use crate::util::{fmt_clock, fmt_ms, fmt_rate};

pub const TICK: Duration = Duration::from_millis(250);
const WINDOW_TICKS: usize = 4;
/// 60 s of sparkline at four points per second.
pub const SPARK_POINTS: usize = 240;

struct Bucket {
    start: Instant,
    end: Instant,
    data: Vec<Interval>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct TimelineRegion {
    pub tps: f64,
    /// Commanded rate (req/s) or active workers.
    pub rate: f64,
    pub in_flight: usize,
    pub p50_ms: Option<f64>,
    pub p99_ms: Option<f64>,
    pub db_p50_ms: Option<f64>,
    pub errors: u64,
    pub throttled: u64,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct TimelinePoint {
    pub t: f64,
    pub total_tps: f64,
    pub regions: Vec<TimelineRegion>,
}

pub struct Monitor {
    window: VecDeque<Bucket>,
    spare: Vec<Vec<Interval>>,
    win: Vec<Interval>,
    win_all: Interval,
    win_secs: f64,
    pub totals: Vec<Interval>,
    pub timeline: Vec<TimelinePoint>,
    spark: VecDeque<u64>,
    pub peak_total: f64,
    pub peak: Vec<f64>,
    pub max_in_flight: Vec<usize>,
    last: Instant,
    ticks: u64,
    pub db_seen: bool,
}

/// Fixed facts for the display.
#[derive(Clone)]
pub struct ViewInfo {
    pub title: String,
    pub duration: Option<f64>,
    pub mix: String,
    pub http: String,
}

pub struct RegionRow {
    pub label: String,
    pub tps: f64,
    pub rate: f64,
    pub in_flight: usize,
    pub p50: Option<f64>,
    pub p90: Option<f64>,
    pub p99: Option<f64>,
    pub db50: Option<f64>,
    pub db99: Option<f64>,
    /// Since the start.
    pub errors: [u64; NCLASS],
    /// In the last second.
    pub recent: [u64; NCLASS],
    pub backing_off: bool,
    pub skipped: u64,
}

pub struct View {
    pub info: ViewInfo,
    pub mode: Mode,
    pub elapsed: f64,
    pub total_tps: f64,
    pub target: f64,
    pub rate: f64,
    pub ramp_left: f64,
    pub backing_off: bool,
    /// Some(in-flight) while draining after stop.
    pub stopping: Option<usize>,
    pub spark: Vec<u64>,
    pub peak: f64,
    pub regions: Vec<RegionRow>,
    pub all: RegionRow,
    pub events: Vec<String>,
    pub show_db: bool,
    pub posts: usize,
    pub users: usize,
}

fn new_intervals(n: usize) -> Vec<Interval> {
    (0..n).map(|_| Interval::new()).collect()
}

impl Monitor {
    pub fn new(n: usize, start: Instant) -> Self {
        Self {
            window: VecDeque::with_capacity(WINDOW_TICKS + 1),
            spare: Vec::new(),
            win: new_intervals(n),
            win_all: Interval::new(),
            win_secs: 0.0,
            totals: new_intervals(n),
            timeline: Vec::new(),
            spark: VecDeque::with_capacity(SPARK_POINTS),
            peak_total: 0.0,
            peak: vec![0.0; n],
            max_in_flight: vec![0; n],
            last: start,
            ticks: 0,
            db_seen: false,
        }
    }

    /// Drain the stats shards. Returns true on whole-second boundaries.
    pub fn tick(&mut self, ctx: &Ctx, now: Instant) -> bool {
        let n = self.win.len();
        let mut data = self.spare.pop().unwrap_or_else(|| new_intervals(n));
        ctx.stats.drain_into(&mut data);
        for (tot, d) in self.totals.iter_mut().zip(&data) {
            tot.add(d);
        }
        self.window.push_back(Bucket {
            start: self.last,
            end: now,
            data,
        });
        self.last = now;
        while self.window.len() > WINDOW_TICKS {
            if let Some(mut old) = self.window.pop_front() {
                old.data.iter_mut().for_each(Interval::reset);
                self.spare.push(old.data);
            }
        }

        self.win.iter_mut().for_each(Interval::reset);
        self.win_all.reset();
        for b in &self.window {
            for (w, d) in self.win.iter_mut().zip(&b.data) {
                w.add(d);
            }
        }
        for w in &self.win {
            self.win_all.add(w);
        }
        self.win_secs = match (self.window.front(), self.window.back()) {
            (Some(f), Some(b)) => b.end.duration_since(f.start).as_secs_f64(),
            _ => 0.0,
        };
        self.db_seen |= !self.win_all.db.is_empty();

        let total = self.total_tps();
        if self.spark.len() == SPARK_POINTS {
            self.spark.pop_front();
        }
        self.spark.push_back(total.round() as u64);
        if self.window.len() == WINDOW_TICKS {
            self.peak_total = self.peak_total.max(total);
            for t in 0..n {
                self.peak[t] = self.peak[t].max(self.tps(t));
            }
        }
        for (t, target) in ctx.targets.iter().enumerate() {
            self.max_in_flight[t] = self.max_in_flight[t].max(target.in_flight());
        }
        self.ticks += 1;
        self.ticks.is_multiple_of(WINDOW_TICKS as u64)
    }

    pub fn tps(&self, t: usize) -> f64 {
        if self.win_secs <= 0.0 {
            0.0
        } else {
            self.win[t].ok() as f64 / self.win_secs
        }
    }

    pub fn total_tps(&self) -> f64 {
        (0..self.win.len()).map(|t| self.tps(t)).sum()
    }

    /// The last second of results for target `t`.
    pub fn window(&self, t: usize) -> &Interval {
        &self.win[t]
    }

    pub fn totals_all(&self) -> Interval {
        let mut all = Interval::new();
        for t in &self.totals {
            all.add(t);
        }
        all
    }

    fn rate_now(ctx: &Ctx, t: usize, secs: f64) -> f64 {
        match ctx.mode {
            Mode::Rate => ctx.controller(t).rate(secs),
            Mode::Concurrency => ctx.active[t].0.load(std::sync::atomic::Ordering::Relaxed) as f64,
        }
    }

    pub fn record_second(&mut self, ctx: &Ctx, secs: f64) {
        let regions = (0..self.win.len())
            .map(|t| {
                let w = &self.win[t];
                TimelineRegion {
                    tps: round1(self.tps(t)),
                    rate: round1(Self::rate_now(ctx, t, secs)),
                    in_flight: ctx.targets[t].in_flight(),
                    p50_ms: pct_ms(&w.lat, 0.5),
                    p99_ms: pct_ms(&w.lat, 0.99),
                    db_p50_ms: pct_ms(&w.db, 0.5),
                    errors: w.errors(),
                    throttled: w.count(Class::Throttled),
                }
            })
            .collect();
        self.timeline.push(TimelinePoint {
            t: round1(secs),
            total_tps: round1(self.total_tps()),
            regions,
        });
    }

    pub fn plain_line(&self, ctx: &Ctx, secs: f64) -> String {
        let mut target = 0.0;
        let mut rate = 0.0;
        for t in 0..self.win.len() {
            target += ctx.controller(t).target();
            rate += Self::rate_now(ctx, t, secs);
        }
        let unit = match ctx.mode {
            Mode::Rate => "/s",
            Mode::Concurrency => " workers",
        };
        let mut line = format!(
            "[{:>6}] TPS {:>7} | rate {}{unit} of {}{unit}",
            fmt_clock(secs),
            fmt_rate(self.total_tps()),
            fmt_rate(rate),
            fmt_rate(target)
        );
        for (t, target) in ctx.targets.iter().enumerate() {
            let w = &self.win[t];
            let tot = &self.totals[t];
            line.push_str(&format!(
                " | {} {}/s in-flight {} p50/p90/p99 {}/{}/{} ms",
                target.label,
                fmt_rate(self.tps(t)),
                target.in_flight(),
                fmt_ms(pct_ms(&w.lat, 0.5)),
                fmt_ms(pct_ms(&w.lat, 0.9)),
                fmt_ms(pct_ms(&w.lat, 0.99)),
            ));
            if self.db_seen {
                line.push_str(&format!(
                    " db {}/{} ms",
                    fmt_ms(pct_ms(&w.db, 0.5)),
                    fmt_ms(pct_ms(&w.db, 0.99))
                ));
            }
            line.push_str(" err");
            for c in Class::ERRORS {
                line.push_str(&format!(" {}={}", c.label(), tot.count(c)));
            }
            let skipped = target.skipped.load(std::sync::atomic::Ordering::Relaxed);
            if skipped > 0 {
                line.push_str(&format!(" skipped={skipped}"));
            }
        }
        line
    }

    fn row(&self, ctx: &Ctx, t: usize, secs: f64) -> RegionRow {
        let w = &self.win[t];
        let c = ctx.controller(t);
        RegionRow {
            label: ctx.targets[t].label.clone(),
            tps: self.tps(t),
            rate: match ctx.mode {
                Mode::Rate => c.rate(secs),
                Mode::Concurrency => {
                    ctx.active[t].0.load(std::sync::atomic::Ordering::Relaxed) as f64
                }
            },
            in_flight: ctx.targets[t].in_flight(),
            p50: pct_ms(&w.lat, 0.5),
            p90: pct_ms(&w.lat, 0.9),
            p99: pct_ms(&w.lat, 0.99),
            db50: pct_ms(&w.db, 0.5),
            db99: pct_ms(&w.db, 0.99),
            errors: self.totals[t].counts,
            recent: w.counts,
            backing_off: c.backing_off(),
            skipped: ctx.targets[t]
                .skipped
                .load(std::sync::atomic::Ordering::Relaxed),
        }
    }

    pub fn view(&self, ctx: &Ctx, info: &ViewInfo, now: Instant, stopping: Option<usize>) -> View {
        let secs = ctx.secs(now);
        let regions: Vec<RegionRow> = (0..self.win.len())
            .map(|t| self.row(ctx, t, secs))
            .collect();
        let mut errors = [0u64; NCLASS];
        let mut recent = [0u64; NCLASS];
        for r in &regions {
            for i in 0..NCLASS {
                errors[i] += r.errors[i];
                recent[i] += r.recent[i];
            }
        }
        let mut target = 0.0;
        let mut ramp_left: f64 = 0.0;
        for t in 0..self.win.len() {
            let c = ctx.controller(t);
            target += c.target();
            ramp_left = ramp_left.max(c.ramp_remaining(secs));
        }
        let all = RegionRow {
            label: "all".into(),
            tps: regions.iter().map(|r| r.tps).sum(),
            rate: regions.iter().map(|r| r.rate).sum(),
            in_flight: regions.iter().map(|r| r.in_flight).sum(),
            p50: pct_ms(&self.win_all.lat, 0.5),
            p90: pct_ms(&self.win_all.lat, 0.9),
            p99: pct_ms(&self.win_all.lat, 0.99),
            db50: pct_ms(&self.win_all.db, 0.5),
            db99: pct_ms(&self.win_all.db, 0.99),
            errors,
            recent,
            backing_off: regions.iter().any(|r| r.backing_off),
            skipped: regions.iter().map(|r| r.skipped).sum(),
        };
        let mut spark: Vec<u64> = vec![0; SPARK_POINTS - self.spark.len()];
        spark.extend(self.spark.iter().copied());
        View {
            info: info.clone(),
            mode: ctx.mode,
            elapsed: secs,
            total_tps: all.tps,
            target,
            rate: all.rate,
            ramp_left,
            backing_off: all.backing_off,
            stopping,
            spark,
            peak: self.peak_total,
            regions,
            all,
            events: ctx.events.recent(20),
            show_db: self.db_seen,
            posts: ctx.pool.post_count(),
            users: ctx.pool.user_count(),
        }
    }
}

fn round1(v: f64) -> f64 {
    (v * 10.0).round() / 10.0
}
