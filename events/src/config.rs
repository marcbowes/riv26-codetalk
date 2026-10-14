//! Working out which stream to read and where to start.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use chrono::{DateTime, Local};
use serde::Deserialize;

/// The parts of `demo/outputs.json` (written by `demo/deploy.sh`) this tool reads.
#[derive(Debug, Deserialize)]
pub struct Outputs {
    pub events: Option<StreamOutput>,
    #[serde(rename = "demoCdc")]
    pub demo_cdc: Option<StreamOutput>,
}

#[derive(Debug, Deserialize)]
pub struct StreamOutput {
    pub region: Option<String>,
    #[serde(rename = "streamName")]
    pub stream_name: Option<String>,
    #[serde(rename = "streamArn")]
    pub stream_arn: Option<String>,
}

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
    /// From `--region`, the ARN, or outputs.json. `None` means the SDK's default chain.
    pub region: Option<String>,
    /// Where the stream came from, for the startup banner.
    pub origin: String,
}

/// Candidate locations for outputs.json when `--outputs` isn't given: `../demo/outputs.json`
/// (running from `events/`), `demo/outputs.json` (from the repo root), then next to this crate.
fn default_outputs() -> Option<PathBuf> {
    let crate_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    [
        PathBuf::from("../demo/outputs.json"),
        PathBuf::from("demo/outputs.json"),
        crate_dir.join("../demo/outputs.json"),
    ]
    .into_iter()
    .find(|p| p.is_file())
}

pub fn resolve_target(
    stream: Option<&str>,
    region: Option<&str>,
    outputs: Option<&Path>,
    demo_cluster: bool,
) -> Result<Target> {
    if let Some(s) = stream {
        let stream = StreamRef::parse(s);
        let region = region
            .map(str::to_owned)
            .or_else(|| stream.arn_region().map(str::to_owned));
        return Ok(Target {
            stream,
            region,
            origin: "--stream".into(),
        });
    }
    let path = match outputs {
        Some(p) => {
            if !p.is_file() {
                bail!("--outputs {}: no such file", p.display());
            }
            p.to_path_buf()
        }
        None => default_outputs().ok_or_else(|| {
            anyhow!(
                "no stream given: pass --stream <name|arn>, or --outputs <path> \
                 (../demo/outputs.json doesn't exist yet; demo/deploy.sh writes it)"
            )
        })?,
    };
    let text =
        std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    target_from_outputs(&text, region, demo_cluster)
        .with_context(|| format!("in {}", path.display()))
}

pub fn target_from_outputs(text: &str, region: Option<&str>, demo_cluster: bool) -> Result<Target> {
    let outputs: Outputs = serde_json::from_str(text).context("parsing outputs.json")?;
    let (key, out) = if demo_cluster {
        (
            "demoCdc",
            outputs
                .demo_cdc
                .context("no demoCdc section (deploy.sh only writes it when CDC is enabled on the demo cluster)")?,
        )
    } else {
        ("events", outputs.events.context("no events section")?)
    };
    let stream = match (out.stream_arn, out.stream_name) {
        (Some(arn), _) if !arn.is_empty() => StreamRef {
            name: None,
            arn: Some(arn),
        },
        (_, Some(name)) if !name.is_empty() => StreamRef {
            name: Some(name),
            arn: None,
        },
        _ => bail!("{key} has neither streamArn nor streamName"),
    };
    let region = region
        .map(str::to_owned)
        .or(out.region)
        .or_else(|| stream.arn_region().map(str::to_owned));
    Ok(Target {
        stream,
        region,
        origin: format!("outputs.json {key}"),
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

    const OUTPUTS: &str = r#"{
      "witnessRegion": "us-west-2",
      "regions": [{"region": "us-east-1", "apiUrl": "https://x.lambda-url.us-east-1.on.aws/"}],
      "webUrl": "https://dxxxx.cloudfront.net",
      "events": { "region": "us-east-1", "streamName": "dat401-cdc",
                  "streamArn": "arn:aws:kinesis:us-east-1:111122223333:stream/dat401-cdc", "cdcRoleArn": "r" },
      "demoCdc": { "region": "us-east-2", "streamName": "dat401-demo-cdc", "streamArn": "", "streamId": "s" }
    }"#;

    #[test]
    fn outputs_events_stream() {
        let t = target_from_outputs(OUTPUTS, None, false).unwrap();
        assert_eq!(
            t.stream.arn.as_deref(),
            Some("arn:aws:kinesis:us-east-1:111122223333:stream/dat401-cdc")
        );
        assert_eq!(t.stream.display_name(), "dat401-cdc");
        assert_eq!(t.region.as_deref(), Some("us-east-1"));
        assert_eq!(t.origin, "outputs.json events");
    }

    #[test]
    fn outputs_demo_cluster_stream() {
        let t = target_from_outputs(OUTPUTS, Some("eu-west-1"), true).unwrap();
        assert_eq!(
            t.stream.name.as_deref(),
            Some("dat401-demo-cdc"),
            "empty ARN falls back to name"
        );
        assert_eq!(t.region.as_deref(), Some("eu-west-1"), "--region wins");
    }

    #[test]
    fn outputs_without_demo_cdc() {
        let err =
            target_from_outputs(r#"{"events": {"streamName": "x"}}"#, None, true).unwrap_err();
        assert!(format!("{err:#}").contains("demoCdc"));
    }

    #[test]
    fn explicit_stream() {
        let t = resolve_target(
            Some("arn:aws:kinesis:us-east-2:1:stream/s"),
            None,
            None,
            false,
        )
        .unwrap();
        assert_eq!(t.region.as_deref(), Some("us-east-2"));
        assert_eq!(t.stream.display_name(), "s");
        let t = resolve_target(Some("my-stream"), None, None, false).unwrap();
        assert_eq!(
            (t.stream.name.as_deref(), t.region),
            (Some("my-stream"), None)
        );
        assert!(
            resolve_target(
                None,
                None,
                Some(Path::new("/nonexistent/outputs.json")),
                false
            )
            .is_err()
        );
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
