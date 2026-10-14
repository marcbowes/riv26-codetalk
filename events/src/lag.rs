//! Per-stage lag for one change.
//!
//! ```text
//!   commit ──commit→CDC──▶ CDC processed ──CDC→Kinesis──▶ Kinesis arrival ──Kinesis→here──▶ shown
//!   source.ts_ns           ts_ns                          ApproximateArrivalTimestamp      local clock
//! ```
//!
//! The first two stages are differences between AWS-side clocks. The last stage and the total
//! use this machine's clock, so they're only as good as local clock sync.

use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    CommitToCdc,
    CdcToKinesis,
    KinesisToHere,
    Total,
}

impl Stage {
    pub const ALL: [Stage; 4] = [
        Stage::CommitToCdc,
        Stage::CdcToKinesis,
        Stage::KinesisToHere,
        Stage::Total,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Stage::CommitToCdc => "commit→CDC",
            Stage::CdcToKinesis => "CDC→Kinesis",
            Stage::KinesisToHere => "Kinesis→here",
            Stage::Total => "total",
        }
    }

    pub fn index(self) -> usize {
        self as usize
    }
}

/// Lag in ns for each stage. A stage is `None` when one of its timestamps is missing.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Lag {
    pub commit_to_cdc: Option<i64>,
    pub cdc_to_kinesis: Option<i64>,
    pub kinesis_to_here: Option<i64>,
    pub total: Option<i64>,
}

impl Lag {
    /// All arguments are ns since the Unix epoch.
    pub fn compute(
        commit: Option<i64>,
        cdc: Option<i64>,
        arrival: Option<i64>,
        here: Option<i64>,
    ) -> Lag {
        let diff = |later: Option<i64>, earlier: Option<i64>| Some(later?.saturating_sub(earlier?));
        Lag {
            commit_to_cdc: diff(cdc, commit),
            cdc_to_kinesis: diff(arrival, cdc),
            kinesis_to_here: diff(here, arrival),
            total: diff(here, commit),
        }
    }

    pub fn get(&self, stage: Stage) -> Option<i64> {
        match stage {
            Stage::CommitToCdc => self.commit_to_cdc,
            Stage::CdcToKinesis => self.cdc_to_kinesis,
            Stage::KinesisToHere => self.kinesis_to_here,
            Stage::Total => self.total,
        }
    }
}

pub fn now_ns() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_nanos()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

/// Compact, at most 6 characters for any value under 100 hours: `0.4ms`, `412ms`, `1.25s`,
/// `12.3s`, `2m03s`, `1h02m`. Negative values (clock skew) keep their sign.
pub fn fmt_ns(ns: i64) -> String {
    if ns < 0 {
        return format!("-{}", fmt_ns(ns.checked_neg().unwrap_or(i64::MAX)));
    }
    let ms = ns as f64 / 1e6;
    if ms < 9.95 {
        format!("{ms:.1}ms")
    } else if ms < 999.5 {
        format!("{ms:.0}ms")
    } else if ms < 9_995.0 {
        format!("{:.2}s", ms / 1e3)
    } else if ms < 59_950.0 {
        format!("{:.1}s", ms / 1e3)
    } else {
        let secs = ns / 1_000_000_000;
        if secs < 3600 {
            format!("{}m{:02}s", secs / 60, secs % 60)
        } else {
            format!("{}h{:02}m", secs / 3600, (secs % 3600) / 60)
        }
    }
}

/// MillisBehindLatest and other whole-millisecond values.
pub fn fmt_ms(ms: i64) -> String {
    if (0..1000).contains(&ms) {
        format!("{ms}ms")
    } else {
        fmt_ns(ms.saturating_mul(1_000_000))
    }
}

pub fn fmt_opt(ns: Option<i64>) -> String {
    ns.map(fmt_ns).unwrap_or_else(|| "—".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: i64 = 1_000_000;

    #[test]
    fn stages_from_doc_example() {
        // INSERT example: commit 1705318200000000000, CDC 1705318200125483291.
        let commit = 1_705_318_200_000_000_000;
        let cdc = 1_705_318_200_125_483_291;
        let arrival = 1_705_318_200_163_000_000; // Kinesis reports ms precision
        let here = 1_705_318_200_411_250_000;
        let lag = Lag::compute(Some(commit), Some(cdc), Some(arrival), Some(here));
        assert_eq!(lag.commit_to_cdc, Some(125_483_291));
        assert_eq!(lag.cdc_to_kinesis, Some(37_516_709));
        assert_eq!(lag.kinesis_to_here, Some(248_250_000));
        assert_eq!(lag.total, Some(411_250_000));
        // The stages add up to the total exactly.
        let sum: i64 = [
            Stage::CommitToCdc,
            Stage::CdcToKinesis,
            Stage::KinesisToHere,
        ]
        .iter()
        .map(|s| lag.get(*s).unwrap())
        .sum();
        assert_eq!(sum, lag.total.unwrap());
    }

    #[test]
    fn missing_timestamps_leave_stages_empty() {
        // Replaying bare CDC records: no Kinesis arrival, no read time.
        let lag = Lag::compute(Some(0), Some(100 * MS), None, None);
        assert_eq!(lag.commit_to_cdc, Some(100 * MS));
        assert_eq!(
            (lag.cdc_to_kinesis, lag.kinesis_to_here, lag.total),
            (None, None, None)
        );

        let lag = Lag::compute(Some(0), None, Some(50 * MS), Some(80 * MS));
        assert_eq!(lag.commit_to_cdc, None);
        assert_eq!(lag.cdc_to_kinesis, None);
        assert_eq!(lag.kinesis_to_here, Some(30 * MS));
        assert_eq!(lag.total, Some(80 * MS));
    }

    #[test]
    fn negative_stage_is_kept() {
        // ApproximateArrivalTimestamp is truncated to ms, so it can land a hair before ts_ns.
        let lag = Lag::compute(Some(0), Some(10_400_000), Some(10 * MS), Some(20 * MS));
        assert_eq!(lag.cdc_to_kinesis, Some(-400_000));
        assert_eq!(fmt_ns(-400_000), "-0.4ms");
    }

    #[test]
    fn does_not_overflow() {
        let lag = Lag::compute(Some(i64::MIN), Some(i64::MAX), None, Some(i64::MAX));
        assert_eq!(lag.commit_to_cdc, Some(i64::MAX));
        assert_eq!(fmt_ns(i64::MIN).chars().next(), Some('-'));
    }

    #[test]
    fn formatting() {
        assert_eq!(fmt_ns(0), "0.0ms");
        assert_eq!(fmt_ns(420_000), "0.4ms");
        assert_eq!(fmt_ns(4_210_000), "4.2ms");
        assert_eq!(fmt_ns(9_960_000), "10ms");
        assert_eq!(fmt_ns(412 * MS), "412ms");
        assert_eq!(fmt_ns(999 * MS), "999ms");
        assert_eq!(fmt_ns(999_600_000), "1.00s");
        assert_eq!(fmt_ns(1_254 * MS), "1.25s");
        assert_eq!(fmt_ns(12_340 * MS), "12.3s");
        assert_eq!(fmt_ns(123_000 * MS), "2m03s");
        assert_eq!(fmt_ns(3_720_000 * MS), "1h02m");
        assert_eq!(fmt_opt(None), "—");
        assert_eq!(fmt_ms(0), "0ms");
        assert_eq!(fmt_ms(250), "250ms");
        assert_eq!(fmt_ms(61_000), "1m01s");
        for ns in [
            0,
            1,
            9_999_999,
            999_999_999,
            59_999_999_999,
            359_999_000_000_000,
        ] {
            assert!(fmt_ns(ns).chars().count() <= 6, "{ns} → {}", fmt_ns(ns));
        }
    }
}
