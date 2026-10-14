//! Load control: a linear ramp, a throttle-driven back-off ceiling (AIMD), and
//! the open-loop pacer.
//!
//! The same `Controller` drives both modes. In open-loop mode its unit is
//! requests per second; in closed-loop mode it is the number of active workers.
//!
//! Back-off follows the riv25 helper's rule: only throttling slows us down.
//! 5xx, timeouts and connect errors are recorded but never change the rate,
//! because the point of the demo is to show that nothing needs to be nursed.

/// Back off when at least this fraction of a second's responses were throttled.
pub const THROTTLE_TRIGGER: f64 = 0.005;
/// Multiplicative decrease: the new ceiling is this fraction of the current rate.
pub const BACKOFF_FACTOR: f64 = 0.85;
/// After a back-off, hold the ceiling for this long before probing upwards.
pub const HOLD_SECS: f64 = 3.0;
/// Additive increase per second while recovering, as a fraction of the target.
pub const RECOVER_PER_SEC: f64 = 0.02;
/// The pacer never catches up more than this much time in one go (after a stall).
pub const MAX_CATCHUP_SECS: f64 = 0.1;

/// A linear segment from `from` (at time `t0`) to `to`, moving at `slope` units/s.
#[derive(Debug, Clone, Copy)]
pub struct Ramp {
    t0: f64,
    from: f64,
    to: f64,
    slope: f64,
}

impl Ramp {
    pub fn new(t0: f64, from: f64, to: f64, slope: f64) -> Self {
        Self {
            t0,
            from,
            to,
            slope,
        }
    }

    fn instant(&self) -> bool {
        !self.slope.is_finite() || self.slope <= 0.0
    }

    pub fn at(&self, t: f64) -> f64 {
        if self.instant() {
            return self.to;
        }
        let delta = self.slope * (t - self.t0).max(0.0);
        if self.to >= self.from {
            (self.from + delta).min(self.to)
        } else {
            (self.from - delta).max(self.to)
        }
    }

    /// Time at which the ramp reaches `to`.
    pub fn end(&self) -> f64 {
        if self.instant() {
            self.t0
        } else {
            self.t0 + (self.to - self.from).abs() / self.slope
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Adjust {
    /// Throttling seen: the rate was cut from `from` to `to`.
    BackedOff {
        from: f64,
        to: f64,
        throttled: u64,
        completed: u64,
    },
    /// The ceiling grew back to the ramp and was removed.
    Recovered { rate: f64 },
}

#[derive(Debug, Clone)]
pub struct Controller {
    ramp: Ramp,
    /// Slope of the original ramp; re-targets (the +/- keys) reuse it.
    slope: f64,
    ceiling: Option<f64>,
    hold_until: f64,
    floor: f64,
    backoffs: u64,
}

impl Controller {
    /// Ramp from zero to `target` over `ramp_secs` (0 = start at the target).
    /// `floor` is the lowest rate a back-off may cut to.
    pub fn new(target: f64, ramp_secs: f64, floor: f64) -> Self {
        let slope = if ramp_secs > 0.0 {
            target / ramp_secs
        } else {
            f64::INFINITY
        };
        Self {
            ramp: Ramp::new(0.0, 0.0, target, slope),
            slope,
            ceiling: None,
            hold_until: 0.0,
            floor,
            backoffs: 0,
        }
    }

    pub fn target(&self) -> f64 {
        self.ramp.to
    }

    /// The rate to use at time `t` (seconds since start).
    pub fn rate(&self, t: f64) -> f64 {
        let r = self.ramp.at(t);
        match self.ceiling {
            Some(c) => r.min(c),
            None => r,
        }
    }

    pub fn ramp_remaining(&self, t: f64) -> f64 {
        (self.ramp.end() - t).max(0.0)
    }

    pub fn backing_off(&self) -> bool {
        self.ceiling.is_some()
    }

    pub fn backoffs(&self) -> u64 {
        self.backoffs
    }

    /// Feed one second of results. `completed` includes the throttled ones.
    pub fn observe(&mut self, t: f64, throttled: u64, completed: u64) -> Option<Adjust> {
        let current = self.rate(t);
        let threshold = (completed as f64 * THROTTLE_TRIGGER).max(1.0);
        if throttled > 0 && throttled as f64 >= threshold {
            let to = (current * BACKOFF_FACTOR).max(self.floor);
            self.ceiling = Some(to);
            self.hold_until = t + HOLD_SECS;
            self.backoffs += 1;
            return Some(Adjust::BackedOff {
                from: current,
                to,
                throttled,
                completed,
            });
        }
        if let Some(c) = self.ceiling
            && t >= self.hold_until
        {
            let next = c + (self.target() * RECOVER_PER_SEC).max(self.floor);
            let ramp = self.ramp.at(t);
            if next >= ramp {
                self.ceiling = None;
                return Some(Adjust::Recovered { rate: ramp });
            }
            self.ceiling = Some(next);
        }
        None
    }

    /// Move to a new target from wherever we are now, at the original slope.
    pub fn set_target(&mut self, t: f64, target: f64) {
        let current = self.rate(t);
        self.ramp = Ramp::new(t, current, target.max(0.0), self.slope);
        self.ceiling = None;
    }
}

/// Open-loop pacing: turns a rate and elapsed time into a whole number of
/// requests to send now, carrying the fractional remainder forward.
#[derive(Debug, Default)]
pub struct Pacer {
    credit: f64,
}

impl Pacer {
    pub fn advance(&mut self, rate: f64, dt: f64) -> u64 {
        let dt = dt.clamp(0.0, MAX_CATCHUP_SECS);
        self.credit += rate.max(0.0) * dt;
        // The epsilon absorbs float drift from summing many tiny increments.
        let n = (self.credit + 1e-9).floor();
        self.credit -= n;
        n as u64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64, tol: f64) -> bool {
        (a - b).abs() <= tol
    }

    #[test]
    fn ramp_is_linear_then_holds() {
        let c = Controller::new(1000.0, 10.0, 1.0);
        assert_eq!(c.rate(0.0), 0.0);
        assert!(close(c.rate(2.5), 250.0, 1e-9));
        assert!(close(c.rate(5.0), 500.0, 1e-9));
        assert_eq!(c.rate(10.0), 1000.0);
        assert_eq!(c.rate(1000.0), 1000.0);
        assert!(close(c.ramp_remaining(4.0), 6.0, 1e-9));
        assert_eq!(c.ramp_remaining(11.0), 0.0);
    }

    #[test]
    fn zero_ramp_starts_at_target() {
        let c = Controller::new(500.0, 0.0, 1.0);
        assert_eq!(c.rate(0.0), 500.0);
        assert_eq!(c.ramp_remaining(0.0), 0.0);
    }

    /// Simulate the pacer with fixed ticks; return the number of requests sent.
    fn simulate(c: &Controller, secs: f64, tick: f64) -> u64 {
        let mut p = Pacer::default();
        let mut t = 0.0;
        let mut sent = 0;
        while t < secs - 1e-12 {
            t += tick;
            // Midpoint rate, as the integral of a linear ramp would give.
            sent += p.advance(c.rate(t - tick / 2.0), tick);
        }
        sent
    }

    #[test]
    fn pacer_sends_exactly_the_rate() {
        let c = Controller::new(1000.0, 0.0, 1.0);
        let sent = simulate(&c, 10.0, 0.001);
        assert!((9_999..=10_000).contains(&sent), "sent {sent}");
    }

    #[test]
    fn pacer_handles_fractional_rates() {
        let c = Controller::new(0.5, 0.0, 0.1);
        assert_eq!(simulate(&c, 10.0, 0.001), 5);
    }

    #[test]
    fn pacer_integrates_the_ramp() {
        let c = Controller::new(1000.0, 10.0, 1.0);
        // Area under the ramp: half of 10 s x 1000/s.
        let ramp_only = simulate(&c, 10.0, 0.001);
        assert!(close(ramp_only as f64, 5000.0, 2.0), "sent {ramp_only}");
        let with_hold = simulate(&c, 20.0, 0.001);
        assert!(close(with_hold as f64, 15000.0, 2.0), "sent {with_hold}");
    }

    #[test]
    fn pacer_tolerates_irregular_ticks() {
        let mut rng = fastrand::Rng::with_seed(7);
        let mut p = Pacer::default();
        let mut elapsed = 0.0;
        let mut sent = 0;
        while elapsed < 20.0 {
            let dt = 0.0005 + rng.f64() * 0.004;
            elapsed += dt;
            sent += p.advance(2000.0, dt);
        }
        let expected = 2000.0 * elapsed;
        assert!(
            close(sent as f64, expected, 1.0),
            "sent {sent}, expected {expected}"
        );
    }

    #[test]
    fn pacer_limits_catch_up_after_a_stall() {
        let mut p = Pacer::default();
        // A 5 s stall at 1000/s must not release 5000 requests at once.
        assert_eq!(p.advance(1000.0, 5.0), 100);
    }

    #[test]
    fn only_throttling_backs_off() {
        let mut c = Controller::new(1000.0, 0.0, 1.0);
        // A second full of 5xx/timeouts reports throttled = 0: no change.
        assert_eq!(c.observe(1.0, 0, 1000), None);
        assert_eq!(c.rate(1.0), 1000.0);
        assert!(!c.backing_off());

        let adj = c.observe(2.0, 50, 1000).expect("backs off");
        assert_eq!(
            adj,
            Adjust::BackedOff {
                from: 1000.0,
                to: 850.0,
                throttled: 50,
                completed: 1000
            }
        );
        assert_eq!(c.rate(2.0), 850.0);
        assert_eq!(c.backoffs(), 1);
    }

    #[test]
    fn sporadic_throttles_are_ignored() {
        let mut c = Controller::new(10_000.0, 0.0, 1.0);
        // 3 of 10,000 is below the 0.5% trigger.
        assert_eq!(c.observe(1.0, 3, 10_000), None);
        assert_eq!(c.rate(1.0), 10_000.0);
        // At low rates a single throttle is enough.
        let mut slow = Controller::new(50.0, 0.0, 1.0);
        assert!(slow.observe(1.0, 1, 50).is_some());
    }

    #[test]
    fn holds_then_recovers_additively() {
        let mut c = Controller::new(1000.0, 0.0, 1.0);
        c.observe(10.0, 100, 1000);
        assert_eq!(c.rate(10.0), 850.0);
        // Held for HOLD_SECS.
        assert_eq!(c.observe(11.0, 0, 850), None);
        assert_eq!(c.observe(12.0, 0, 850), None);
        assert_eq!(c.rate(12.0), 850.0);
        // Then +2% of target per second.
        assert_eq!(c.observe(13.0, 0, 850), None);
        assert!(close(c.rate(13.0), 870.0, 1e-9));
        let mut t = 13.0;
        let mut recovered = None;
        while recovered.is_none() && t < 100.0 {
            t += 1.0;
            recovered = c.observe(t, 0, 900);
        }
        assert_eq!(recovered, Some(Adjust::Recovered { rate: 1000.0 }));
        assert!(!c.backing_off());
        assert_eq!(c.rate(t), 1000.0);
        // 150 to make up at 20/s: 8 steps after the hold.
        assert!(close(t, 20.0, 1e-9), "recovered at {t}");
    }

    #[test]
    fn repeated_backoff_compounds_and_respects_floor() {
        let mut c = Controller::new(100.0, 0.0, 10.0);
        for i in 0..40 {
            c.observe(i as f64, 100, 100);
        }
        assert_eq!(c.rate(40.0), 10.0);
        assert_eq!(c.backoffs(), 40);
    }

    #[test]
    fn backoff_during_ramp_then_ramp_continues() {
        let mut c = Controller::new(1000.0, 10.0, 1.0);
        c.observe(5.0, 20, 500); // cut 500 -> 425
        assert_eq!(c.rate(5.0), 425.0);
        assert_eq!(c.rate(8.0), 425.0); // ceiling holds while the ramp moves on
        let mut t = 5.0;
        while c.backing_off() {
            t += 1.0;
            c.observe(t, 0, 400);
        }
        // Recovery is additive (20/s from 425), so it ends well after the ramp did.
        assert!(t > 30.0, "recovered at {t}");
        assert_eq!(c.rate(t), 1000.0);
    }

    #[test]
    fn retarget_reuses_the_ramp_slope() {
        let mut c = Controller::new(1000.0, 10.0, 1.0); // 100/s
        c.set_target(20.0, 2000.0);
        assert!(close(c.rate(25.0), 1500.0, 1e-9));
        assert_eq!(c.rate(30.0), 2000.0);
        assert_eq!(c.target(), 2000.0);
        c.set_target(40.0, 500.0);
        assert!(close(c.rate(41.0), 1900.0, 1e-9));
        assert_eq!(c.rate(55.0), 500.0);
        // Re-targeting clears a back-off ceiling.
        c.observe(60.0, 10, 10);
        assert!(c.backing_off());
        c.set_target(61.0, 600.0);
        assert!(!c.backing_off());
    }

    #[test]
    fn retarget_with_zero_ramp_is_instant() {
        let mut c = Controller::new(100.0, 0.0, 1.0);
        c.set_target(5.0, 300.0);
        assert_eq!(c.rate(5.0), 300.0);
    }
}
