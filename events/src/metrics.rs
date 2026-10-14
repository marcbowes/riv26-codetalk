//! Rolling counters and lag percentiles shared by the views.
//!
//! Everything is bucketed by whole seconds since start (monotonic clock). Percentiles come from
//! one HdrHistogram per second, merged over the window on demand.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::Arc;
use std::time::Instant;

use hdrhistogram::Histogram;

use crate::lag::{Lag, Stage};
use crate::pipeline::Change;
use crate::record::Op;

/// Histograms track µs up to an hour with 2 significant figures (~1% error).
const HIST_MAX_US: u64 = 3_600_000_000;
const RING: usize = 64;

fn new_hist() -> Histogram<u64> {
    Histogram::new_with_bounds(1, HIST_MAX_US, 2).expect("valid histogram bounds")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Quantiles {
    pub p50: i64,
    pub p99: i64,
    pub max: i64,
    pub count: u64,
}

struct Slot {
    sec: i64,
    hist: Histogram<u64>,
    max_ns: i64,
    count: u64,
}

/// Lag distribution over the last N seconds.
pub struct WindowHist {
    slots: Vec<Slot>,
}

impl WindowHist {
    pub fn new(window_secs: usize) -> Self {
        WindowHist {
            slots: (0..window_secs.max(1) + 1)
                .map(|_| Slot {
                    sec: i64::MIN,
                    hist: new_hist(),
                    max_ns: i64::MIN,
                    count: 0,
                })
                .collect(),
        }
    }

    pub fn record(&mut self, sec: i64, ns: i64) {
        let len = self.slots.len() as i64;
        let slot = &mut self.slots[sec.rem_euclid(len) as usize];
        if slot.sec != sec {
            slot.sec = sec;
            slot.hist.reset();
            slot.max_ns = i64::MIN;
            slot.count = 0;
        }
        // Negative lag (clock skew, ms truncation) counts as zero in the percentiles; max is exact.
        slot.hist.saturating_record((ns.max(0) / 1_000) as u64);
        slot.max_ns = slot.max_ns.max(ns);
        slot.count += 1;
    }

    /// Percentiles over the seconds in `(now_sec - window, now_sec]`.
    pub fn summary(&self, now_sec: i64, window: i64) -> Option<Quantiles> {
        let mut merged: Option<Histogram<u64>> = None;
        let (mut max, mut count) = (i64::MIN, 0);
        for s in &self.slots {
            if s.count == 0 || s.sec <= now_sec - window || s.sec > now_sec {
                continue;
            }
            match merged.as_mut() {
                Some(m) => {
                    let _ = m.add(&s.hist);
                }
                None => merged = Some(s.hist.clone()),
            }
            max = max.max(s.max_ns);
            count += s.count;
        }
        let m = merged?;
        let q = |p: f64| {
            (m.value_at_quantile(p) as i64)
                .saturating_mul(1_000)
                .min(max.max(0))
        };
        Some(Quantiles {
            p50: q(0.50),
            p99: q(0.99),
            max,
            count,
        })
    }
}

/// Per-second event counts for the last ~minute.
#[derive(Clone)]
pub struct RateRing {
    secs: [i64; RING],
    counts: [u64; RING],
    pub total: u64,
}

impl Default for RateRing {
    fn default() -> Self {
        RateRing {
            secs: [i64::MIN; RING],
            counts: [0; RING],
            total: 0,
        }
    }
}

impl RateRing {
    pub fn add(&mut self, sec: i64, n: u64) {
        let i = sec.rem_euclid(RING as i64) as usize;
        if self.secs[i] != sec {
            self.secs[i] = sec;
            self.counts[i] = 0;
        }
        self.counts[i] += n;
        self.total += n;
    }

    fn count_at(&self, sec: i64) -> u64 {
        let i = sec.rem_euclid(RING as i64) as usize;
        if self.secs[i] == sec {
            self.counts[i]
        } else {
            0
        }
    }

    /// Mean rate over the last `n` completed seconds (fewer right after start).
    pub fn per_sec(&self, now_sec: i64, n: i64) -> f64 {
        let n = n.clamp(1, RING as i64 - 2).min(now_sec);
        if n <= 0 {
            return 0.0;
        }
        let sum: u64 = (now_sec - n..now_sec).map(|s| self.count_at(s)).sum();
        sum as f64 / n as f64
    }
}

#[derive(Debug, Default, Clone)]
pub struct ShardState {
    pub millis_behind: Option<i64>,
}

pub struct Metrics {
    start: Instant,
    pub window_secs: i64,
    lag: [WindowHist; 4],
    pub changes: RateRing,
    pub by_table_op: BTreeMap<(String, Op), RateRing>,
    /// Raw Kinesis records, including fragments.
    pub records: RateRing,
    pub polls: RateRing,
    pub shards: BTreeMap<Arc<str>, ShardState>,
    pub shards_closed: u64,
    pub throttles: u64,
    pub errors: u64,
    pub last_error: Option<String>,
    pub warnings: VecDeque<String>,
    /// `source.cluster` values seen (a multi-Region cluster's stream reports one ID).
    pub clusters: BTreeSet<String>,
}

impl Metrics {
    pub fn new(window_secs: u64) -> Self {
        let w = window_secs.max(1) as usize;
        Metrics {
            start: Instant::now(),
            window_secs: w as i64,
            lag: std::array::from_fn(|_| WindowHist::new(w)),
            changes: RateRing::default(),
            by_table_op: BTreeMap::new(),
            records: RateRing::default(),
            polls: RateRing::default(),
            shards: BTreeMap::new(),
            shards_closed: 0,
            throttles: 0,
            errors: 0,
            last_error: None,
            warnings: VecDeque::new(),
            clusters: BTreeSet::new(),
        }
    }

    pub fn now_sec(&self) -> i64 {
        self.start.elapsed().as_secs() as i64
    }

    pub fn uptime_secs(&self) -> u64 {
        self.start.elapsed().as_secs()
    }

    pub fn record_change(&mut self, c: &Change) {
        let sec = self.now_sec();
        self.changes.add(sec, 1);
        self.by_table_op
            .entry((c.env.table().to_owned(), c.op))
            .or_default()
            .add(sec, 1);
        self.record_lag(sec, &c.lag);
        if let Some(id) = c.env.cluster()
            && self.clusters.len() < 8
            && !self.clusters.contains(id)
        {
            self.clusters.insert(id.to_owned());
        }
    }

    pub fn record_lag(&mut self, sec: i64, lag: &Lag) {
        for stage in Stage::ALL {
            if let Some(ns) = lag.get(stage) {
                self.lag[stage.index()].record(sec, ns);
            }
        }
    }

    pub fn lag_summary(&self, stage: Stage) -> Option<Quantiles> {
        self.lag[stage.index()].summary(self.now_sec(), self.window_secs)
    }

    pub fn record_poll(&mut self, shard: &Arc<str>, millis_behind: Option<i64>, records: usize) {
        let sec = self.now_sec();
        self.records.add(sec, records as u64);
        let st = self.shards.entry(shard.clone()).or_default();
        if millis_behind.is_some() {
            self.polls.add(sec, 1);
            st.millis_behind = millis_behind;
        }
    }

    pub fn shard_opened(&mut self, shard: &Arc<str>) {
        self.shards.entry(shard.clone()).or_default();
    }

    pub fn shard_closed(&mut self, shard: &Arc<str>) {
        if self.shards.remove(shard).is_some() {
            self.shards_closed += 1;
        }
    }

    /// Max `MillisBehindLatest` over open shards that have reported one.
    pub fn max_behind_ms(&self) -> Option<i64> {
        self.shards.values().filter_map(|s| s.millis_behind).max()
    }

    pub fn warn(&mut self, msg: &str) {
        if self.warnings.len() >= 5 {
            self.warnings.pop_front();
        }
        self.warnings.push_back(msg.to_owned());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: i64 = 1_000_000;

    #[test]
    fn window_quantiles() {
        let mut h = WindowHist::new(10);
        for i in 1..=100 {
            h.record(5, i * 10 * MS); // 10ms..1000ms
        }
        let q = h.summary(5, 10).unwrap();
        assert_eq!(q.count, 100);
        assert!((495 * MS..=510 * MS).contains(&q.p50), "p50 {}", q.p50);
        assert!((985 * MS..=1000 * MS).contains(&q.p99), "p99 {}", q.p99);
        assert_eq!(q.max, 1000 * MS);
        // Still in the window 9s later, gone after 10s.
        assert!(h.summary(14, 10).is_some());
        assert!(h.summary(15, 10).is_none());
    }

    #[test]
    fn window_merges_seconds_and_reuses_slots() {
        let mut h = WindowHist::new(2);
        h.record(0, 100 * MS);
        h.record(1, 300 * MS);
        h.record(2, 200 * MS);
        let q = h.summary(2, 2).unwrap();
        assert_eq!((q.count, q.max), (2, 300 * MS));
        // Slot for second 0 is reused by second 3.
        h.record(3, 5 * MS);
        let q = h.summary(3, 3).unwrap();
        assert_eq!(q.count, 3);
        assert_eq!(h.summary(100, 2), None);
    }

    #[test]
    fn negative_lag_is_clamped_in_percentiles_only() {
        let mut h = WindowHist::new(5);
        h.record(0, -2 * MS);
        let q = h.summary(0, 5).unwrap();
        assert_eq!((q.p50, q.max), (0, -2 * MS));
    }

    #[test]
    fn rates() {
        let mut r = RateRing::default();
        r.add(0, 10);
        r.add(1, 20);
        r.add(2, 30);
        r.add(3, 1000); // current, partial second: not counted
        assert_eq!(r.per_sec(3, 1), 30.0);
        assert_eq!(r.per_sec(3, 3), 20.0);
        assert_eq!(r.per_sec(3, 10), 20.0, "only 3 completed seconds so far");
        assert_eq!(r.per_sec(0, 5), 0.0);
        assert_eq!(r.total, 1060);
        // Old buckets age out.
        assert_eq!(r.per_sec(200, 5), 0.0);
    }

    #[test]
    fn shard_behind() {
        let mut m = Metrics::new(10);
        let (a, b): (Arc<str>, Arc<str>) = ("shardId-0".into(), "shardId-1".into());
        m.shard_opened(&a);
        assert_eq!(m.max_behind_ms(), None);
        m.record_poll(&a, Some(0), 3);
        m.record_poll(&b, Some(1500), 0);
        assert_eq!(m.max_behind_ms(), Some(1500));
        m.shard_closed(&b);
        assert_eq!(m.max_behind_ms(), Some(0));
        assert_eq!((m.shards.len(), m.shards_closed), (1, 1));
    }
}
