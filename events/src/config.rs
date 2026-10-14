//! Working out which stream to read and where to start.

use std::time::Duration;

use anyhow::Result;
use chrono::{DateTime, Local};

/// A Kinesis stream, by name or ARN (exactly one is set).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamRef {
    pub name: Option<String>,
    pub arn: Option<String>,
}

impl StreamRef {
    pub fn parse(s: &str) -> StreamRef {
        if s.starts_with("arn:") {
            StreamRef {
                name: None,
                arn: Some(s.to_owned()),
            }
        } else {
            StreamRef {
                name: Some(s.to_owned()),
                arn: None,
            }
        }
    }

    pub fn display_name(&self) -> &str {
        match (&self.name, &self.arn) {
            (Some(n), _) => n,
            (None, Some(arn)) => arn.rsplit_once("stream/").map(|(_, n)| n).unwrap_or(arn),
            (None, None) => "?",
        }
    }

    /// `arn:aws:kinesis:<region>:<account>:stream/<name>` → region.
    pub fn arn_region(&self) -> Option<&str> {
        let arn = self.arn.as_deref()?;
        arn.split(':').nth(3).filter(|r| !r.is_empty())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub stream: StreamRef,
    /// From `--region` or the ARN. `None` means the SDK's default chain.
    pub region: Option<String>,
    /// Where the stream came from, for the startup banner.
    pub origin: String,
}

pub fn resolve_target(stream: &str, region: Option<&str>) -> Result<Target> {
    let stream = StreamRef::parse(stream);
    let region = region
        .map(str::to_owned)
        .or_else(|| stream.arn_region().map(str::to_owned));
    Ok(Target {
        stream,
        region,
        origin: "--stream".into(),
    })
}

/// `--from`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartPos {
    Latest,
    TrimHorizon,
    /// AT_TIMESTAMP, ms since the epoch (resolved once at startup so every shard agrees).
    AtTimestamp(i64),
}

impl StartPos {
    pub fn parse(s: &str) -> Result<StartPos, String> {
        let t = s.trim().to_ascii_lowercase();
        match t.as_str() {
            "latest" | "now" => return Ok(StartPos::Latest),
            "trim-horizon" | "trim_horizon" | "earliest" | "start" => {
                return Ok(StartPos::TrimHorizon);
            }
            _ => {}
        }
        if let Ok(d) = parse_duration(&t) {
            let ago = chrono::Duration::from_std(d).map_err(|e| e.to_string())?;
            return Ok(StartPos::AtTimestamp(
                (chrono::Utc::now() - ago).timestamp_millis(),
            ));
        }
        if let Ok(dt) = DateTime::parse_from_rfc3339(s.trim()) {
            return Ok(StartPos::AtTimestamp(dt.timestamp_millis()));
        }
        Err(format!(
            "expected latest, trim-horizon, a lookback like 5m or 1h30m, or an RFC 3339 time; got {s:?}"
        ))
    }

    pub fn describe(&self) -> String {
        match self {
            StartPos::Latest => "latest".into(),
            StartPos::TrimHorizon => "trim horizon".into(),
            StartPos::AtTimestamp(ms) => match DateTime::from_timestamp_millis(*ms) {
                Some(dt) => format!("{}", dt.with_timezone(&Local).format("%H:%M:%S")),
                None => ms.to_string(),
            },
        }
    }
}

/// `250ms`, `30s`, `5m`, `1h30m`, `2d`. A bare number is seconds.
pub fn parse_duration(s: &str) -> Result<Duration, String> {
    let s = s.trim();
    if s.is_empty() {
        return Err("empty duration".into());
    }
    if let Ok(n) = s.parse::<f64>() {
        return if n >= 0.0 && n.is_finite() {
            Ok(Duration::from_secs_f64(n))
        } else {
            Err(format!("bad duration {s:?}"))
        };
    }
    let mut total = Duration::ZERO;
    let mut rest = s;
    while !rest.is_empty() {
        let num_len = rest
            .find(|c: char| !(c.is_ascii_digit() || c == '.'))
            .unwrap_or(rest.len());
        let unit_len = rest[num_len..]
            .find(|c: char| c.is_ascii_digit())
            .unwrap_or(rest.len() - num_len);
        let (num, unit) = (&rest[..num_len], &rest[num_len..num_len + unit_len]);
        let n: f64 = num.parse().map_err(|_| format!("bad duration {s:?}"))?;
        let secs = match unit {
            "ms" => n / 1000.0,
            "s" | "sec" | "secs" => n,
            "m" | "min" | "mins" => n * 60.0,
            "h" | "hr" | "hrs" => n * 3600.0,
            "d" | "day" | "days" => n * 86_400.0,
            _ => {
                return Err(format!(
                    "bad duration unit {unit:?} in {s:?} (use ms, s, m, h or d)"
                ));
            }
        };
        total += Duration::from_secs_f64(secs);
        rest = &rest[num_len + unit_len..];
    }
    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_stream() {
        let t = resolve_target("arn:aws:kinesis:us-east-2:1:stream/s", None).unwrap();
        assert_eq!(t.region.as_deref(), Some("us-east-2"));
        assert_eq!(t.stream.display_name(), "s");
        let t = resolve_target("my-stream", None).unwrap();
        assert_eq!(
            (t.stream.name.as_deref(), t.region),
            (Some("my-stream"), None)
        );
        let t = resolve_target("arn:aws:kinesis:us-east-2:1:stream/s", Some("eu-west-1")).unwrap();
        assert_eq!(t.region.as_deref(), Some("eu-west-1"), "--region wins");
    }

    #[test]
    fn durations() {
        assert_eq!(parse_duration("5m"), Ok(Duration::from_secs(300)));
        assert_eq!(parse_duration("1h30m"), Ok(Duration::from_secs(5400)));
        assert_eq!(parse_duration("250ms"), Ok(Duration::from_millis(250)));
        assert_eq!(parse_duration("1.5s"), Ok(Duration::from_millis(1500)));
        assert_eq!(parse_duration("90"), Ok(Duration::from_secs(90)));
        assert_eq!(parse_duration("2d"), Ok(Duration::from_secs(172_800)));
        assert!(parse_duration("5 parsecs").is_err());
        assert!(parse_duration("m").is_err());
        assert!(parse_duration("").is_err());
    }

    #[test]
    fn start_positions() {
        assert_eq!(StartPos::parse("latest"), Ok(StartPos::Latest));
        assert_eq!(StartPos::parse("TRIM-HORIZON"), Ok(StartPos::TrimHorizon));
        let before = chrono::Utc::now().timestamp_millis();
        let StartPos::AtTimestamp(ms) = StartPos::parse("5m").unwrap() else {
            panic!()
        };
        assert!((before - 300_000 - ms).abs() < 5_000);
        assert_eq!(
            StartPos::parse("2026-10-14T12:00:00Z"),
            Ok(StartPos::AtTimestamp(1_791_979_200_000))
        );
        assert!(StartPos::parse("yesterday").is_err());
    }
}
