//! Per-request accounting.
//!
//! Every completed request is recorded into a shard picked by the recording
//! thread, so tokio workers almost never contend on a lock. The monitor drains
//! all shards four times a second.

use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use hdrhistogram::Histogram;

use crate::mix::{NOPS, Op};

/// Outcome classes. Throttling is kept apart from other errors because it is
/// the only thing that makes the controller back off.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Class {
    Ok,
    Client4xx,
    Throttled,
    Server5xx,
    Timeout,
    Connect,
}

pub const NCLASS: usize = 6;

impl Class {
    pub const ERRORS: [Class; NCLASS - 1] = [
        Class::Client4xx,
        Class::Throttled,
        Class::Server5xx,
        Class::Timeout,
        Class::Connect,
    ];

    pub fn index(self) -> usize {
        self as usize
    }

    pub fn label(self) -> &'static str {
        match self {
            Class::Ok => "2xx",
            Class::Client4xx => "4xx",
            Class::Throttled => "429",
            Class::Server5xx => "5xx",
            Class::Timeout => "timeout",
            Class::Connect => "connect",
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Class::Ok => "ok",
            Class::Client4xx => "client4xx",
            Class::Throttled => "throttled",
            Class::Server5xx => "server5xx",
            Class::Timeout => "timeout",
            Class::Connect => "connect",
        }
    }
}

/// Classify an HTTP response. `error_type` is the `x-amzn-ErrorType` header,
/// which Lambda sets on its own errors (e.g. `TooManyRequestsException`).
pub fn classify_status(status: u16, error_type: Option<&[u8]>) -> Class {
    if (200..300).contains(&status) {
        return Class::Ok;
    }
    if let Some(et) = error_type
        && (memchr::memmem::find(et, b"TooManyRequests").is_some()
            || memchr::memmem::find(et, b"Throttl").is_some()
            || memchr::memmem::find(et, b"throttl").is_some())
    {
        return Class::Throttled;
    }
    match status {
        429 => Class::Throttled,
        500..=599 => Class::Server5xx,
        _ => Class::Client4xx,
    }
}

/// Latencies are recorded in microseconds, up to two minutes.
pub const HIST_MAX_US: u64 = 120_000_000;

pub fn new_hist() -> Histogram<u64> {
    Histogram::new_with_bounds(1, HIST_MAX_US, 2).expect("valid histogram bounds")
}

/// Percentile in milliseconds, or None if nothing was recorded.
pub fn pct_ms(h: &Histogram<u64>, q: f64) -> Option<f64> {
    if h.is_empty() {
        None
    } else {
        Some(h.value_at_quantile(q) as f64 / 1000.0)
    }
}

pub fn max_ms(h: &Histogram<u64>) -> Option<f64> {
    if h.is_empty() {
        None
    } else {
        Some(h.max() as f64 / 1000.0)
    }
}

/// Counts and histograms for one target over some interval.
#[derive(Clone)]
pub struct Interval {
    pub counts: [u64; NCLASS],
    pub op_counts: [[u64; NCLASS]; NOPS],
    /// Latency of successful responses (µs).
    pub lat: Histogram<u64>,
    /// Server-reported `timing.dbMs` of successful responses (µs).
    pub db: Histogram<u64>,
    /// Latency of successful responses per op (µs).
    pub op_lat: Vec<Histogram<u64>>,
}

impl Default for Interval {
    fn default() -> Self {
        Self::new()
    }
}

impl Interval {
    pub fn new() -> Self {
        Self {
            counts: [0; NCLASS],
            op_counts: [[0; NCLASS]; NOPS],
            lat: new_hist(),
            db: new_hist(),
            op_lat: (0..NOPS).map(|_| new_hist()).collect(),
        }
    }

    pub fn record(&mut self, op: Op, class: Class, latency_us: u64, db_us: Option<u64>) {
        self.counts[class.index()] += 1;
        self.op_counts[op.index()][class.index()] += 1;
        if class == Class::Ok {
            let us = latency_us.max(1);
            self.lat.saturating_record(us);
            self.op_lat[op.index()].saturating_record(us);
            if let Some(db) = db_us {
                self.db.saturating_record(db.max(1));
            }
        }
    }

    pub fn add(&mut self, other: &Interval) {
        for (a, b) in self.counts.iter_mut().zip(other.counts) {
            *a += b;
        }
        for (a, b) in self.op_counts.iter_mut().zip(other.op_counts) {
            for (x, y) in a.iter_mut().zip(b) {
                *x += y;
            }
        }
        add_hist(&mut self.lat, &other.lat);
        add_hist(&mut self.db, &other.db);
        for (a, b) in self.op_lat.iter_mut().zip(&other.op_lat) {
            add_hist(a, b);
        }
    }

    pub fn reset(&mut self) {
        self.counts = [0; NCLASS];
        self.op_counts = [[0; NCLASS]; NOPS];
        for h in [&mut self.lat, &mut self.db]
            .into_iter()
            .chain(self.op_lat.iter_mut())
        {
            if !h.is_empty() {
                h.reset();
            }
        }
    }

    pub fn total(&self) -> u64 {
        self.counts.iter().sum()
    }

    pub fn count(&self, c: Class) -> u64 {
        self.counts[c.index()]
    }

    pub fn ok(&self) -> u64 {
        self.count(Class::Ok)
    }

    pub fn errors(&self) -> u64 {
        self.total() - self.ok()
    }

    pub fn op_total(&self, op: Op) -> u64 {
        self.op_counts[op.index()].iter().sum()
    }
}

fn add_hist(a: &mut Histogram<u64>, b: &Histogram<u64>) {
    if !b.is_empty() {
        a.add(b).expect("histograms share bounds");
    }
}

#[repr(align(128))]
struct Shard(Mutex<Vec<Interval>>);

static NEXT_SHARD: AtomicUsize = AtomicUsize::new(0);

thread_local! {
    static SHARD_ID: usize = NEXT_SHARD.fetch_add(1, Ordering::Relaxed);
}

pub struct Stats {
    shards: Box<[Shard]>,
    mask: usize,
}

impl Stats {
    pub fn new(targets: usize, threads: usize) -> Self {
        let n = threads.max(1).next_power_of_two();
        let shards = (0..n)
            .map(|_| Shard(Mutex::new((0..targets).map(|_| Interval::new()).collect())))
            .collect();
        Self {
            shards,
            mask: n - 1,
        }
    }

    pub fn record(&self, target: usize, op: Op, class: Class, latency_us: u64, db_us: Option<u64>) {
        let i = SHARD_ID.with(|id| *id) & self.mask;
        let mut shard = self.shards[i].0.lock().unwrap_or_else(|e| e.into_inner());
        shard[target].record(op, class, latency_us, db_us);
    }

    /// Add everything recorded since the last drain into `out`, and reset.
    pub fn drain_into(&self, out: &mut [Interval]) {
        for shard in self.shards.iter() {
            let mut shard = shard.0.lock().unwrap_or_else(|e| e.into_inner());
            for (o, s) in out.iter_mut().zip(shard.iter_mut()) {
                if s.total() > 0 {
                    o.add(s);
                    s.reset();
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_statuses() {
        assert_eq!(classify_status(200, None), Class::Ok);
        assert_eq!(classify_status(201, None), Class::Ok);
        assert_eq!(classify_status(404, None), Class::Client4xx);
        assert_eq!(classify_status(400, None), Class::Client4xx);
        assert_eq!(classify_status(429, None), Class::Throttled);
        assert_eq!(classify_status(500, None), Class::Server5xx);
        assert_eq!(classify_status(502, None), Class::Server5xx);
        // Lambda's own throttle signal wins over the status code.
        assert_eq!(
            classify_status(503, Some(b"TooManyRequestsException")),
            Class::Throttled
        );
        assert_eq!(
            classify_status(400, Some(b"ThrottlingException")),
            Class::Throttled
        );
        assert_eq!(
            classify_status(403, Some(b"AccessDeniedException")),
            Class::Client4xx
        );
    }

    #[test]
    fn records_and_drains_across_threads() {
        let stats = std::sync::Arc::new(Stats::new(2, 4));
        let handles: Vec<_> = (0..8)
            .map(|i| {
                let stats = stats.clone();
                std::thread::spawn(move || {
                    for j in 0..1000u64 {
                        let class = if j % 10 == 0 {
                            Class::Throttled
                        } else {
                            Class::Ok
                        };
                        stats.record(i % 2, Op::GetPost, class, 1000 + j, Some(200));
                    }
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
        let mut out = vec![Interval::new(), Interval::new()];
        stats.drain_into(&mut out);
        for o in &out {
            assert_eq!(o.total(), 4000);
            assert_eq!(o.count(Class::Throttled), 400);
            assert_eq!(o.lat.len(), 3600);
            assert_eq!(o.db.len(), 3600);
            assert_eq!(o.op_total(Op::GetPost), 4000);
            let p50 = pct_ms(&o.lat, 0.5).unwrap();
            assert!((1.4..1.6).contains(&p50), "p50 {p50}");
        }
        // Drained shards are empty.
        let mut again = vec![Interval::new(), Interval::new()];
        stats.drain_into(&mut again);
        assert_eq!(again[0].total() + again[1].total(), 0);
    }

    #[test]
    fn latency_only_counts_successes() {
        let mut i = Interval::new();
        i.record(Op::ListPosts, Class::Timeout, 10_000_000, None);
        i.record(Op::ListPosts, Class::Ok, 5_000, Some(1_000));
        assert_eq!(i.lat.len(), 1);
        assert_eq!(i.errors(), 1);
        // Two significant figures: within 1%.
        let p99 = pct_ms(&i.lat, 0.99).unwrap();
        assert!((4.95..5.05).contains(&p99), "{p99}");
        let mut sum = Interval::new();
        sum.add(&i);
        sum.add(&i);
        assert_eq!(sum.total(), 4);
        sum.reset();
        assert_eq!(sum.total(), 0);
        assert!(sum.lat.is_empty());
    }
}
