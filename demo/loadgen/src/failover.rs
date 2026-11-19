//! Region failover (open loop): when most of a Region's requests fail, send its
//! traffic to the healthy Regions instead, keeping a trickle of probes going to
//! it, and move the traffic back once the probes succeed again.
//!
//! This is what a multi-Region client does when one Region's API goes away (in
//! the talk, an FIS experiment makes us-east-1's DSQL endpoint refuse
//! connections, so its API answers 503 at once). The total stays at the target
//! while the healthy Region takes the lot.
//!
//! What counts: 5xx, timeouts, connection errors and throttling (429). 4xx is
//! the request's fault. A little throttling is `control`'s back-off; a Region
//! answering mostly 429 is as gone as one answering 503. (When FIS makes a
//! Region's DSQL refuse connections, every request there tries a new one, and
//! the refused attempts use up the cluster's 100 new connections a second: most
//! answers are 429 "rate exceeded", the rest 503.)
//!
//! States, per Region, judged once a second on that second's responses:
//! - **Up**: keeps all its own traffic, and takes rerouted traffic.
//! - **Down**: at least `DOWN_FRACTION` of at least `DOWN_MIN_COMPLETED`
//!   responses failed. It keeps `PROBE_SHARE` of its own traffic (at least
//!   `PROBE_MIN_RATE` req/s) as probes; the rest goes to the other Regions that
//!   are Up or returning.
//! - **Returning**: `UP_SECONDS` seconds in a row of probes at least
//!   `UP_OK_FRACTION` OK (with at least `UP_MIN_COMPLETED` each). Its share
//!   ramps from the probe share back to all of it over `RETURN_SECS`, so its
//!   Lambda environments reconnect to DSQL a few at a time (well under 100 a
//!   second at 10,000 req/s), not all at once. Hard failures (5xx, timeouts,
//!   connection errors) send it straight back to Down; throttling on the way
//!   back is left to `control`'s back-off.

use std::sync::Mutex;
use std::sync::atomic::Ordering::Relaxed;
use std::sync::atomic::{AtomicBool, AtomicU64};

use crate::stats::{Class, Interval};
use crate::util::fmt_int;

/// A Region is down when at least this fraction of a second's responses failed...
pub const DOWN_FRACTION: f64 = 0.5;
/// ...out of at least this many.
pub const DOWN_MIN_COMPLETED: u64 = 10;
/// While down, this share of the Region's own traffic still goes to it, as probes...
pub const PROBE_SHARE: f64 = 0.002;
/// ...and at least this many requests a second, so a second nearly always
/// has the `UP_MIN_COMPLETED` probes it needs to count.
pub const PROBE_MIN_RATE: f64 = 5.0;
/// A second of probes counts as good when at least this fraction were OK...
pub const UP_OK_FRACTION: f64 = 0.9;
/// ...out of at least this many.
pub const UP_MIN_COMPLETED: u64 = 2;
/// Good seconds in a row before traffic starts moving back.
pub const UP_SECONDS: u32 = 3;
/// How long the move back takes, from the probe share to all of it.
pub const RETURN_SECS: f64 = 30.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum State {
    Up,
    Down,
    /// Moving traffic back, since this time (seconds since start).
    Returning {
        since: f64,
    },
}

/// A state change worth an event.
#[derive(Debug, Clone, PartialEq)]
pub enum Change {
    /// Went down: `failed` of `completed` responses in the last second failed,
    /// mostly of class `worst`.
    Down {
        failed: u64,
        completed: u64,
        worst: Class,
        /// Some failures were throttles (429) and some weren't.
        mixed: bool,
    },
    /// Probes are healthy again; traffic starts moving back.
    Returning,
}

/// The state machine for one Region.
#[derive(Debug, Clone)]
pub struct Health {
    state: State,
    good_streak: u32,
    downs: u64,
}

impl Default for Health {
    fn default() -> Self {
        Self::new()
    }
}

impl Health {
    pub fn new() -> Self {
        Self {
            state: State::Up,
            good_streak: 0,
            downs: 0,
        }
    }

    pub fn state(&self) -> State {
        self.state
    }

    /// Times this Region has gone down.
    pub fn downs(&self) -> u64 {
        self.downs
    }

    /// Feed one second of this Region's results (seconds since start `t`).
    pub fn observe(&mut self, t: f64, w: &Interval) -> Option<Change> {
        let completed = w.total();
        let count = |cs: &[Class]| cs.iter().map(|&c| w.count(c)).sum::<u64>();
        let failed = count(&FAILURES);
        let hard = count(&HARD_FAILURES);
        let enough = completed >= DOWN_MIN_COMPLETED;
        let failing = enough && failed as f64 >= completed as f64 * DOWN_FRACTION;
        let failing_hard = enough && hard as f64 >= completed as f64 * DOWN_FRACTION;
        let down_now = match self.state {
            State::Up => failing,
            State::Returning { .. } => failing_hard,
            State::Down => false,
        };
        match self.state {
            State::Up | State::Returning { .. } if down_now => {
                self.state = State::Down;
                self.good_streak = 0;
                self.downs += 1;
                let worst = FAILURES
                    .iter()
                    .copied()
                    .max_by_key(|&c| w.count(c))
                    .unwrap_or(Class::Server5xx);
                let throttled = w.count(Class::Throttled);
                Some(Change::Down {
                    failed,
                    completed,
                    worst,
                    mixed: throttled > 0 && throttled < failed,
                })
            }
            State::Down => {
                let good = completed >= UP_MIN_COMPLETED
                    && w.ok() as f64 >= completed as f64 * UP_OK_FRACTION;
                self.good_streak = if good { self.good_streak + 1 } else { 0 };
                if self.good_streak >= UP_SECONDS {
                    self.state = State::Returning { since: t };
                    self.good_streak = 0;
                    return Some(Change::Returning);
                }
                None
            }
            State::Returning { since } if t >= since + RETURN_SECS => {
                self.state = State::Up;
                None
            }
            _ => None,
        }
    }

    /// Fraction of this Region's own traffic it keeps at time `t`, when its own
    /// rate is `rate` req/s. The rest is for the other Regions.
    pub fn keep(&self, t: f64, rate: f64) -> f64 {
        let probe = probe_share(rate);
        match self.state {
            State::Up => 1.0,
            State::Down => probe,
            State::Returning { since } => {
                let done = ((t - since) / RETURN_SECS).clamp(0.0, 1.0);
                probe + (1.0 - probe) * done
            }
        }
    }

    /// Can this Region take another Region's traffic?
    pub fn receives(&self) -> bool {
        self.state != State::Down
    }
}

/// What makes a Region down. `HARD_FAILURES`, without throttling, is what
/// sends a returning Region back.
const FAILURES: [Class; 4] = [Class::Server5xx, Class::Throttled, Class::Timeout, Class::Connect];
const HARD_FAILURES: [Class; 3] = [Class::Server5xx, Class::Timeout, Class::Connect];

fn probe_share(rate: f64) -> f64 {
    if rate <= 0.0 {
        return 1.0;
    }
    PROBE_SHARE.max(PROBE_MIN_RATE / rate).min(1.0)
}

/// Each Region's effective sending rate, given its own `rates`, the share of
/// its traffic it `keep`s, and which Regions take rerouted traffic (`receives`).
/// A Region's rerouted share is spread evenly over the other receivers; with
/// none, it keeps everything.
pub fn effective_rates(rates: &[f64], keep: &[f64], receives: &[bool]) -> Vec<f64> {
    let n = rates.len();
    let mut out: Vec<f64> = (0..n).map(|t| rates[t] * keep[t]).collect();
    for d in 0..n {
        let moved = rates[d] * (1.0 - keep[d]);
        if moved <= 0.0 {
            continue;
        }
        let to: Vec<usize> = (0..n).filter(|&u| u != d && receives[u]).collect();
        if to.is_empty() {
            out[d] += moved;
        } else {
            for &u in &to {
                out[u] += moved / to.len() as f64;
            }
        }
    }
    out
}

/// Failover for every Region: the state machines (updated once a second) and
/// what the pacers read on every send (published four times a second).
pub struct Failover {
    enabled: bool,
    health: Vec<Mutex<Health>>,
    /// f64 bits: the share of each Region's own traffic it keeps.
    keep: Vec<AtomicU64>,
    receives: Vec<AtomicBool>,
}

impl Failover {
    pub fn new(n: usize, enabled: bool) -> Self {
        Self {
            enabled,
            health: (0..n).map(|_| Mutex::new(Health::new())).collect(),
            keep: (0..n).map(|_| AtomicU64::new(1.0f64.to_bits())).collect(),
            receives: (0..n).map(|_| AtomicBool::new(true)).collect(),
        }
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    fn health(&self, t: usize) -> std::sync::MutexGuard<'_, Health> {
        self.health[t].lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn state(&self, t: usize) -> State {
        self.health(t).state()
    }

    pub fn downs(&self, t: usize) -> u64 {
        self.health(t).downs()
    }

    /// Feed one second of Region `t`'s results.
    pub fn observe(&self, t: usize, secs: f64, w: &Interval) -> Option<Change> {
        if !self.enabled {
            return None;
        }
        self.health(t).observe(secs, w)
    }

    /// Recompute what the pacers read, for Regions sending `rates` now.
    pub fn publish(&self, secs: f64, rates: &[f64]) {
        if !self.enabled {
            return;
        }
        let receives: Vec<bool> = (0..rates.len())
            .map(|t| self.health(t).receives())
            .collect();
        for (t, rate) in rates.iter().enumerate() {
            let others = (0..rates.len()).any(|u| u != t && receives[u]);
            let keep = if others {
                self.health(t).keep(secs, *rate)
            } else {
                1.0
            };
            self.keep[t].store(keep.to_bits(), Relaxed);
            self.receives[t].store(receives[t], Relaxed);
        }
    }

    fn keep(&self, t: usize) -> f64 {
        f64::from_bits(self.keep[t].load(Relaxed))
    }

    /// Where a request that Region `from`'s pacer is about to send should go.
    pub fn route(&self, from: usize) -> usize {
        let keep = self.keep(from);
        if keep >= 1.0 || fastrand::f64() < keep {
            return from;
        }
        let n = self.receives.len();
        let ok = |u: usize| u != from && self.receives[u].load(Relaxed);
        let count = (0..n).filter(|&u| ok(u)).count();
        if count == 0 {
            return from;
        }
        (0..n)
            .filter(|&u| ok(u))
            .nth(fastrand::usize(..count))
            .unwrap_or(from)
    }

    /// Effective sending rates after rerouting, from each Region's own `rates`.
    pub fn effective(&self, rates: &[f64]) -> Vec<f64> {
        if !self.enabled {
            return rates.to_vec();
        }
        let keep: Vec<f64> = (0..rates.len()).map(|t| self.keep(t)).collect();
        let receives: Vec<bool> = (0..rates.len())
            .map(|t| self.receives[t].load(Relaxed))
            .collect();
        effective_rates(rates, &keep, &receives)
    }

    /// The Regions that took Region `t`'s traffic when it went down.
    pub fn receivers_of(&self, t: usize) -> Vec<usize> {
        (0..self.health.len())
            .filter(|&u| u != t && self.health(u).receives())
            .collect()
    }
}

/// An event line for `change` in Region `t`.
pub fn describe(labels: &[String], t: usize, change: &Change, receivers: &[usize]) -> String {
    let label = &labels[t];
    match change {
        Change::Down {
            failed,
            completed,
            worst,
            mixed,
        } => {
            let pct = (*failed as f64 / (*completed).max(1) as f64 * 100.0).round();
            let what = match (worst, mixed) {
                (Class::Server5xx | Class::Throttled, true) => "failing (5xx and 429)",
                (Class::Throttled, false) => "failing (429)",
                (Class::Timeout, _) => "timing out",
                (Class::Connect, _) => "failing to connect",
                _ => "failing (5xx)",
            };
            let head = format!("{label}: {pct}% of {} requests {what}", fmt_int(*completed));
            if receivers.is_empty() {
                format!("{head}; no other Region is healthy, so not rerouting")
            } else {
                let to: Vec<&str> = receivers.iter().map(|&u| labels[u].as_str()).collect();
                format!("{head}; sending its traffic to {}", to.join(" and "))
            }
        }
        Change::Returning => format!(
            "{label} answering again; moving its traffic back over {} s",
            RETURN_SECS
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mix::Op;

    fn second(ok: u64, failed: u64, class: Class) -> Interval {
        let mut w = Interval::new();
        for _ in 0..ok {
            w.record(Op::GetPost, Class::Ok, 1000, Some(100));
        }
        for _ in 0..failed {
            w.record(Op::GetPost, class, 1000, None);
        }
        w
    }

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn goes_down_on_half_failing() {
        let mut h = Health::new();
        assert_eq!(h.observe(1.0, &second(100, 40, Class::Server5xx)), None);
        assert_eq!(h.state(), State::Up);
        let c = h.observe(2.0, &second(10, 90, Class::Server5xx));
        assert_eq!(
            c,
            Some(Change::Down {
                failed: 90,
                completed: 100,
                worst: Class::Server5xx,
                mixed: false,
            })
        );
        assert_eq!(h.state(), State::Down);
        assert_eq!(h.downs(), 1);
    }

    #[test]
    fn needs_enough_responses_to_go_down() {
        let mut h = Health::new();
        assert_eq!(h.observe(1.0, &second(0, 9, Class::Connect)), None);
        assert_eq!(h.state(), State::Up);
        assert!(h.observe(2.0, &second(0, 10, Class::Connect)).is_some());
    }

    #[test]
    fn failures_and_throttles_count_4xx_doesnt() {
        let mut h = Health::new();
        assert_eq!(h.observe(1.0, &second(0, 100, Class::Client4xx)), None);
        assert_eq!(h.state(), State::Up);
        // Timeouts and connection errors count, and name the failure.
        let c = h.observe(2.0, &second(2, 30, Class::Timeout));
        assert!(matches!(
            c,
            Some(Change::Down {
                worst: Class::Timeout,
                ..
            })
        ));
        // A Region answering mostly 429 is down too (FIS: refused attempts use
        // up DSQL's connection rate).
        let mut h = Health::new();
        let c = h.observe(1.0, &second(5, 95, Class::Throttled));
        assert!(matches!(
            c,
            Some(Change::Down {
                worst: Class::Throttled,
                mixed: false,
                ..
            })
        ));
    }

    #[test]
    fn throttling_on_the_way_back_is_left_to_the_back_off() {
        let mut h = Health::new();
        h.observe(1.0, &second(0, 100, Class::Server5xx));
        for t in 2..5 {
            h.observe(t as f64, &second(10, 0, Class::Server5xx));
        }
        assert!(matches!(h.state(), State::Returning { .. }));
        // Reconnecting Lambdas hit DSQL's connection rate: 429s don't send it back...
        assert_eq!(h.observe(6.0, &second(20, 80, Class::Throttled)), None);
        assert!(matches!(h.state(), State::Returning { .. }));
        // ...but failures do.
        assert!(h.observe(7.0, &second(20, 80, Class::Server5xx)).is_some());
        assert_eq!(h.state(), State::Down);
    }

    #[test]
    fn probes_while_down() {
        let mut h = Health::new();
        h.observe(1.0, &second(0, 100, Class::Server5xx));
        assert!(close(h.keep(1.0, 10_000.0), PROBE_SHARE));
        // At low rates the probe floor wins: 5 of 50 req/s.
        assert!(close(h.keep(1.0, 50.0), 0.1));
        assert!(close(h.keep(1.0, 1.0), 1.0));
        assert!(!h.receives());
        assert!(Health::new().receives());
        assert!(close(Health::new().keep(0.0, 10_000.0), 1.0));
    }

    #[test]
    fn comes_back_after_three_good_seconds_and_ramps() {
        let mut h = Health::new();
        h.observe(1.0, &second(0, 100, Class::Server5xx));
        assert_eq!(h.observe(2.0, &second(10, 0, Class::Ok)), None);
        assert_eq!(h.observe(3.0, &second(10, 0, Class::Ok)), None);
        // A bad second resets the streak; too few probes isn't good either.
        assert_eq!(h.observe(4.0, &second(5, 5, Class::Server5xx)), None);
        assert_eq!(h.observe(5.0, &second(1, 0, Class::Ok)), None);
        assert_eq!(h.observe(6.0, &second(19, 1, Class::Server5xx)), None);
        assert_eq!(h.observe(7.0, &second(10, 0, Class::Ok)), None);
        assert_eq!(
            h.observe(8.0, &second(10, 0, Class::Ok)),
            Some(Change::Returning)
        );
        assert_eq!(h.state(), State::Returning { since: 8.0 });
        assert!(h.receives());

        let rate = 10_000.0;
        let half = 8.0 + RETURN_SECS / 2.0;
        let end = 8.0 + RETURN_SECS;
        assert!(close(h.keep(8.0, rate), PROBE_SHARE));
        assert!(close(h.keep(half, rate), PROBE_SHARE + (1.0 - PROBE_SHARE) / 2.0));
        assert!(close(h.keep(end, rate), 1.0));
        assert!(close(h.keep(end + 12.0, rate), 1.0));
        // Fully back after RETURN_SECS.
        assert_eq!(h.observe(end - 1.0, &second(100, 0, Class::Ok)), None);
        assert!(matches!(h.state(), State::Returning { .. }));
        assert_eq!(h.observe(end, &second(100, 0, Class::Ok)), None);
        assert_eq!(h.state(), State::Up);
    }

    #[test]
    fn failing_while_returning_goes_straight_back_down() {
        let mut h = Health::new();
        h.observe(1.0, &second(0, 100, Class::Server5xx));
        for t in 2..=4 {
            h.observe(t as f64, &second(10, 0, Class::Ok));
        }
        assert!(matches!(h.state(), State::Returning { .. }));
        assert!(matches!(
            h.observe(5.0, &second(5, 50, Class::Server5xx)),
            Some(Change::Down { .. })
        ));
        assert_eq!(h.state(), State::Down);
        assert_eq!(h.downs(), 2);
    }

    #[test]
    fn effective_rates_move_traffic_to_receivers() {
        // Two Regions at 10k each; the second is down and keeps 1%.
        let r = effective_rates(&[10_000.0, 10_000.0], &[1.0, 0.01], &[true, false]);
        assert!(close(r[0], 19_900.0) && close(r[1], 100.0), "{r:?}");
        assert!(close(r.iter().sum::<f64>(), 20_000.0));
        // Spread over several receivers.
        let r = effective_rates(
            &[300.0, 300.0, 300.0],
            &[0.0, 1.0, 1.0],
            &[false, true, true],
        );
        assert!(
            close(r[0], 0.0) && close(r[1], 450.0) && close(r[2], 450.0),
            "{r:?}"
        );
        // Nobody to take it: it stays put.
        let r = effective_rates(&[100.0, 100.0], &[0.5, 0.5], &[false, false]);
        assert!(close(r[0], 100.0) && close(r[1], 100.0), "{r:?}");
    }

    #[test]
    fn failover_routes_and_publishes() {
        let f = Failover::new(2, true);
        let rates = [1000.0, 1000.0];
        f.observe(1, 1.0, &second(0, 100, Class::Server5xx));
        f.publish(1.0, &rates);
        assert_eq!(f.receivers_of(1), vec![0]);
        let to_0 = (0..10_000).filter(|_| f.route(1) == 0).count();
        // Keeps the probe floor, 5 req/s of 1,000 (0.5%, more than PROBE_SHARE):
        // about 9,950 of 10,000 rerouted.
        assert!((9_900..=9_990).contains(&to_0), "{to_0}");
        assert!((0..1000).all(|_| f.route(0) == 0));
        let e = f.effective(&rates);
        assert!(close(e[0], 1995.0) && close(e[1], 5.0), "{e:?}");

        // Both down: nobody reroutes.
        f.observe(0, 1.0, &second(0, 100, Class::Server5xx));
        f.publish(1.0, &rates);
        assert!((0..1000).all(|_| f.route(1) == 1));
        assert_eq!(f.effective(&rates), rates.to_vec());
    }

    #[test]
    fn disabled_does_nothing() {
        let f = Failover::new(2, false);
        assert_eq!(f.observe(1, 1.0, &second(0, 100, Class::Server5xx)), None);
        f.publish(1.0, &[100.0, 100.0]);
        assert!((0..1000).all(|_| f.route(1) == 1));
        assert_eq!(f.state(1), State::Up);
    }

    #[test]
    fn describes_changes() {
        let labels = vec!["us-west-2".to_string(), "us-east-1".to_string()];
        let down = Change::Down {
            failed: 970,
            completed: 1000,
            worst: Class::Server5xx,
            mixed: false,
        };
        let mixed = Change::Down {
            failed: 970,
            completed: 1000,
            worst: Class::Throttled,
            mixed: true,
        };
        assert_eq!(
            describe(&labels, 1, &mixed, &[0]),
            "us-east-1: 97% of 1,000 requests failing (5xx and 429); sending its traffic to us-west-2"
        );
        assert_eq!(
            describe(&labels, 1, &down, &[0]),
            "us-east-1: 97% of 1,000 requests failing (5xx); sending its traffic to us-west-2"
        );
        assert_eq!(
            describe(&labels, 1, &down, &[]),
            "us-east-1: 97% of 1,000 requests failing (5xx); no other Region is healthy, so not rerouting"
        );
        assert_eq!(
            describe(&labels, 1, &Change::Returning, &[0]),
            "us-east-1 answering again; moving its traffic back over 30 s"
        );
    }
}
