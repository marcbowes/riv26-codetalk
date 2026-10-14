//! `replay`: feed a JSON-lines file through the same pipeline, offline.
//!
//! Each line is either a bare CDC record (it has a top-level `type`), or a wrapper:
//!
//! ```json
//! {"arrival": "2026-10-14T17:00:01.163Z", "received": "2026-10-14T17:00:01.411Z",
//!  "shard": "shardId-000000000001", "data": { ...CDC record... }}
//! ```
//!
//! `arrival` is the Kinesis ApproximateArrivalTimestamp and `received` is when the reader got
//! the record; both are optional, as RFC 3339 strings or epoch numbers (s, ms, µs or ns, by
//! magnitude). `data` may also be the record's JSON text as a string. The AWS CLI record shape
//! (`Data` base64, `ApproximateArrivalTimestamp`) works too, so
//! `aws kinesis get-records … | jq -c '.Records[]'` output can be replayed. `--save` writes the
//! wrapper format.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use base64::Engine;
use chrono::{DateTime, SecondsFormat, Utc};
use serde_json::{Map, Value};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::mpsc;

use crate::app::SourceMsg;
use crate::pipeline::{Piece, raw_json};
use crate::record::Envelope;

#[derive(Debug, Clone, PartialEq)]
pub struct ReplayLine {
    pub data: Vec<u8>,
    pub arrival_ns: Option<i64>,
    pub received_ns: Option<i64>,
    pub shard: Option<String>,
    /// Best available time for pacing: received, arrival, CDC time, commit time.
    pub event_ns: Option<i64>,
}

/// Epoch number in s, ms, µs or ns (by magnitude), or an RFC 3339 string → ns.
pub fn parse_time(v: &Value) -> Option<i64> {
    // ns per unit, picked by magnitude: < 1e11 is seconds, < 1e14 ms, < 1e17 µs, else ns.
    fn scale(magnitude: f64) -> i64 {
        match magnitude {
            m if m < 1e11 => 1_000_000_000,
            m if m < 1e14 => 1_000_000,
            m if m < 1e17 => 1_000,
            _ => 1,
        }
    }
    match v {
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                return Some(i.saturating_mul(scale(i.unsigned_abs() as f64)));
            }
            let f = n.as_f64()?;
            let s = scale(f.abs());
            // Whole and fractional parts separately: f64 can't hold epoch ns exactly.
            let whole = f.trunc();
            Some((whole as i64).saturating_mul(s) + ((f - whole) * s as f64).round() as i64)
        }
        Value::String(s) => {
            let s = s.trim();
            if let Ok(d) = DateTime::parse_from_rfc3339(s) {
                return d.timestamp_nanos_opt();
            }
            let n = s
                .parse::<i64>()
                .map(Value::from)
                .or_else(|_| s.parse::<f64>().map(Value::from));
            parse_time(&n.ok()?)
        }
        _ => None,
    }
}

pub fn fmt_time(ns: i64) -> String {
    DateTime::<Utc>::from_timestamp_nanos(ns).to_rfc3339_opts(SecondsFormat::AutoSi, true)
}

fn field<'a>(obj: &'a Map<String, Value>, keys: &[&str]) -> Option<&'a Value> {
    keys.iter()
        .find_map(|k| obj.get(*k))
        .filter(|v| !v.is_null())
}

pub fn parse_line(line: &str) -> Result<ReplayLine> {
    let v: Value = serde_json::from_str(line).context("not JSON")?;
    let Some(obj) = v.as_object() else {
        bail!("not a JSON object")
    };

    let (data, arrival_ns, received_ns, shard) = if obj.contains_key("type") {
        (line.as_bytes().to_vec(), None, None, None)
    } else {
        let data = match field(obj, &["data", "Data"]) {
            Some(Value::String(s)) if obj.contains_key("Data") => {
                base64::engine::general_purpose::STANDARD
                    .decode(s.trim())
                    .unwrap_or_else(|_| s.clone().into_bytes())
            }
            Some(Value::String(s)) => s.clone().into_bytes(),
            Some(other @ Value::Object(_)) => other.to_string().into_bytes(),
            _ => bail!("expected a CDC record (with \"type\") or a wrapper with \"data\""),
        };
        let arrival = field(
            obj,
            &[
                "arrival",
                "approximateArrivalTimestamp",
                "ApproximateArrivalTimestamp",
            ],
        )
        .and_then(parse_time);
        let received = field(obj, &["received"]).and_then(parse_time);
        let shard = field(obj, &["shard", "shardId", "ShardId"])
            .and_then(Value::as_str)
            .map(str::to_owned);
        (data, arrival, received, shard)
    };

    let env = Envelope::parse(&data).ok();
    let event_ns = received_ns
        .or(arrival_ns)
        .or_else(|| env.as_ref().and_then(|e| e.cdc_ns().or(e.commit_ns())));
    Ok(ReplayLine {
        data,
        arrival_ns,
        received_ns,
        shard,
        event_ns,
    })
}

/// One `--save` line.
pub fn save_line(
    data: &[u8],
    arrival_ns: Option<i64>,
    received_ns: Option<i64>,
    shard: &str,
) -> String {
    let mut out = String::with_capacity(data.len() + 128);
    out.push('{');
    if let Some(a) = arrival_ns {
        out += &format!("\"arrival\":\"{}\",", fmt_time(a));
    }
    if let Some(r) = received_ns {
        out += &format!("\"received\":\"{}\",", fmt_time(r));
    }
    out += &format!(
        "\"shard\":{},\"data\":{}}}",
        Value::from(shard),
        raw_json(data).get()
    );
    out
}

#[derive(Debug, Clone)]
pub struct ReplayOptions {
    /// 1.0 keeps the recorded pacing, 2.0 is twice as fast, 0 is as fast as possible.
    pub speed: f64,
    /// Longest pause between two records.
    pub max_gap: Duration,
}

pub async fn run(path: &Path, opts: ReplayOptions, tx: mpsc::Sender<SourceMsg>) -> Result<()> {
    let file = tokio::fs::File::open(path)
        .await
        .with_context(|| format!("opening {}", path.display()))?;
    let mut lines = BufReader::new(file).lines();
    let default_shard: Arc<str> = "replay".into();
    let (mut prev_event, mut last_here): (Option<i64>, Option<i64>) = (None, None);
    let mut lineno = 0u64;

    while let Some(line) = lines.next_line().await? {
        lineno += 1;
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let rl = match parse_line(line) {
            Ok(rl) => rl,
            Err(e) => {
                let msg = format!("{}:{lineno}: {e:#}", path.display());
                if tx.send(SourceMsg::Error(msg)).await.is_err() {
                    return Ok(());
                }
                continue;
            }
        };

        if let (Some(t), Some(p)) = (rl.event_ns, prev_event)
            && opts.speed > 0.0
        {
            let gap = Duration::from_nanos(t.saturating_sub(p).max(0) as u64).div_f64(opts.speed);
            let gap = gap.min(opts.max_gap);
            if !gap.is_zero() {
                tokio::time::sleep(gap).await;
            }
        }
        if rl.event_ns.is_some() {
            prev_event = rl.event_ns;
        }
        // Bare fragments carry no times; they were read about when the record before them was.
        let here = rl.received_ns.or(if rl.event_ns.is_none() {
            last_here
        } else {
            None
        });
        if rl.received_ns.is_some() {
            last_here = rl.received_ns;
        }

        let shard = rl
            .shard
            .map(Arc::from)
            .unwrap_or_else(|| default_shard.clone());
        let msg = SourceMsg::Batch {
            shard,
            pieces: vec![Piece {
                data: rl.data,
                arrival_ns: rl.arrival_ns,
                here_ns: here,
            }],
            millis_behind: None,
        };
        if tx.send(msg).await.is_err() {
            return Ok(());
        }
    }
    let _ = tx.send(SourceMsg::End).await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::record::tests::DOC_INSERT;

    #[test]
    fn bare_record_line() {
        let line: String = DOC_INSERT.split_whitespace().collect::<Vec<_>>().join(" ");
        let rl = parse_line(&line).unwrap();
        assert_eq!(rl.data, line.as_bytes());
        assert_eq!(
            (rl.arrival_ns, rl.received_ns, rl.shard.as_deref()),
            (None, None, None)
        );
        assert_eq!(rl.event_ns, Some(1_705_318_200_125_483_291));
    }

    #[test]
    fn wrapper_line() {
        let line = r#"{"arrival":"2024-01-15T11:30:00.163Z","received":1705318200411.25,"shard":"shardId-000000000003",
                       "data":{"type":"fragment","chunk_id":"c","index":0,"data":"{\"a\""}}"#;
        let rl = parse_line(line).unwrap();
        assert_eq!(rl.arrival_ns, Some(1_705_318_200_163_000_000));
        assert_eq!(rl.received_ns, Some(1_705_318_200_411_250_000));
        assert_eq!(rl.shard.as_deref(), Some("shardId-000000000003"));
        assert_eq!(rl.event_ns, rl.received_ns);
        let env = Envelope::parse(&rl.data).unwrap();
        assert_eq!(env.data.as_deref(), Some("{\"a\""));
    }

    #[test]
    fn aws_cli_record_shape() {
        let data = base64::engine::general_purpose::STANDARD.encode(DOC_INSERT);
        let line = format!(
            r#"{{"SequenceNumber":"4959","ApproximateArrivalTimestamp":1705318200.163,"Data":"{data}","PartitionKey":"k"}}"#
        );
        let rl = parse_line(&line).unwrap();
        assert_eq!(rl.data, DOC_INSERT.as_bytes());
        assert_eq!(
            rl.arrival_ns.map(|n| n / 1_000_000),
            Some(1_705_318_200_163)
        );
    }

    #[test]
    fn data_as_json_text() {
        let line = format!(r#"{{"data":{}}}"#, Value::from(DOC_INSERT));
        assert_eq!(parse_line(&line).unwrap().data, DOC_INSERT.as_bytes());
    }

    #[test]
    fn bad_lines() {
        assert!(parse_line("nope").is_err());
        assert!(parse_line("[1,2]").is_err());
        assert!(parse_line(r#"{"hello":"world"}"#).is_err());
    }

    #[test]
    fn times_by_magnitude() {
        let ns = 1_705_318_200_163_000_000i64;
        assert_eq!(
            parse_time(&Value::from(1_705_318_200.163)).map(|n| n / 1_000_000),
            Some(ns / 1_000_000)
        );
        assert_eq!(parse_time(&Value::from(1_705_318_200_163i64)), Some(ns));
        assert_eq!(parse_time(&Value::from(1_705_318_200_163_000i64)), Some(ns));
        assert_eq!(parse_time(&Value::from(ns)), Some(ns));
        assert_eq!(
            parse_time(&Value::from("2024-01-15T11:30:00.163+00:00")),
            Some(ns)
        );
        assert_eq!(parse_time(&Value::from("1705318200163")), Some(ns));
        assert_eq!(parse_time(&Value::Null), None);
    }

    #[test]
    fn shipped_sample_replays_cleanly() {
        use crate::pipeline::{Event, Pipeline, PipelineOptions};
        use crate::record::Op;

        let mut p = Pipeline::new(PipelineOptions {
            tables: vec![],
            keep_raw: true,
            chunk_ttl: Duration::from_secs(120),
        });
        let mut out = Vec::new();
        for line in include_str!("../samples/blog.jsonl")
            .lines()
            .filter(|l| !l.trim().is_empty())
        {
            let rl = parse_line(line).unwrap();
            let piece = Piece {
                data: rl.data,
                arrival_ns: rl.arrival_ns,
                here_ns: rl.received_ns,
            };
            p.push(piece, crate::lag::now_ns(), &mut out);
        }
        p.finish(&mut out);
        let warnings: Vec<_> = out
            .iter()
            .filter(|e| matches!(e, Event::Warning(_)))
            .collect();
        assert!(warnings.is_empty(), "{warnings:?}");
        let changes: Vec<_> = out
            .iter()
            .filter_map(|e| match e {
                Event::Change(c) => Some(c),
                _ => None,
            })
            .collect();
        let summary: Vec<(Op, &str)> = changes.iter().map(|c| (c.op, c.env.table())).collect();
        assert_eq!(
            summary,
            [
                (Op::Insert, "users"),
                (Op::Insert, "users"),
                (Op::Insert, "posts"),
                (Op::Insert, "comments"),
                (Op::Insert, "comments"),
                (Op::Insert, "comments"),
                (Op::Update, "posts"),
                (Op::Delete, "comments"),
                (Op::Insert, "posts"),
                (Op::Insert, "comments"),
            ]
        );
        for c in &changes {
            let l = c.lag;
            assert!(l.total.is_some() && l.commit_to_cdc.is_some() && l.kinesis_to_here.is_some());
            assert!((0..1_000_000_000).contains(&l.total.unwrap()), "{l:?}");
        }
        let big = changes[8];
        assert_eq!(big.fragments, 3);
        assert_eq!(big.env.after_value().unwrap()["slug"], "the-long-read");
        assert_eq!(
            big.raw.as_ref().unwrap()["after"]["title"],
            "The long read: CDC end to end"
        );
        assert_eq!(p.chunk_stats().reassembled, 1);
        assert_eq!(p.chunk_stats().crc_errors + p.chunk_stats().unverified, 0);
        let delete = changes[7];
        assert!(delete.env.before_value().unwrap()["id"].is_string());
    }

    #[test]
    fn save_lines_round_trip() {
        let line = save_line(
            DOC_INSERT.as_bytes(),
            Some(1_705_318_200_163_000_000),
            Some(1_705_318_200_411_250_000),
            "shardId-1",
        );
        assert!(line.starts_with(r#"{"arrival":"2024-01-15T11:30:00.163Z","received":"2024-01-15T11:30:00.411250Z","shard":"shardId-1","data":{"#), "{line}");
        let rl = parse_line(&line).unwrap();
        assert_eq!(rl.arrival_ns, Some(1_705_318_200_163_000_000));
        assert_eq!(rl.received_ns, Some(1_705_318_200_411_250_000));
        let env = Envelope::parse(&rl.data).unwrap();
        assert_eq!(env.cdc_ns(), Some(1_705_318_200_125_483_291));
        // Non-JSON data is saved as a string rather than corrupting the file.
        let odd = save_line(b"\xffnot json", None, None, "s");
        assert!(serde_json::from_str::<Value>(&odd).is_ok());
    }
}
