//! Reading every shard of a Kinesis data stream with plain GetRecords polling.
//!
//! CDC uses random partition keys, so changes (and the fragments of a chunked record) land on
//! any shard. A shard manager re-lists shards periodically so it notices on-demand resharding:
//! it starts a poller for each new shard (from TRIM_HORIZON, since everything in a new shard is
//! new) and a poller stops by itself when its shard is closed and drained.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use aws_sdk_kinesis::Client;
use aws_sdk_kinesis::config::retry::RetryConfig;
use aws_sdk_kinesis::error::{DisplayErrorContext, ProvideErrorMetadata, SdkError};
use aws_sdk_kinesis::primitives::DateTime;
use aws_sdk_kinesis::types::ShardIteratorType;
use tokio::sync::mpsc;
use tokio::time::{Instant, sleep, sleep_until};

use crate::app::SourceMsg;
use crate::config::{StartPos, StreamRef};
use crate::pipeline::Piece;

/// Most records one GetRecords call may return.
const GET_RECORDS_LIMIT: i32 = 10_000;

#[derive(Debug, Clone)]
pub struct StreamInfo {
    pub name: String,
    pub arn: String,
    pub status: String,
    pub mode: String,
    pub open_shards: i32,
    pub retention_hours: i32,
    pub max_record_kib: Option<i32>,
}

/// `Code: message` for service errors; the full chain for transport errors (timeouts, DNS,
/// credentials), where the detail is the useful part.
pub fn sdk_error<E, R>(e: &SdkError<E, R>) -> String
where
    E: ProvideErrorMetadata + std::error::Error + 'static,
    R: std::fmt::Debug,
{
    match e {
        SdkError::ServiceError(_) => match (e.code(), e.message()) {
            (Some(c), Some(m)) => format!("{c}: {m}"),
            (Some(c), None) => c.to_owned(),
            _ => DisplayErrorContext(e).to_string(),
        },
        _ => DisplayErrorContext(e).to_string(),
    }
}

pub async fn describe(client: &Client, stream: &StreamRef) -> Result<StreamInfo> {
    let resp = client
        .describe_stream_summary()
        .set_stream_name(stream.name.clone())
        .set_stream_arn(stream.arn.clone())
        .send()
        .await
        .map_err(|e| {
            let hint = match e.code() {
                Some("ResourceNotFoundException") => {
                    " (create it first, e.g. with demo/deploy.sh, or check --region)"
                }
                _ => "",
            };
            anyhow::anyhow!("{}{hint}", sdk_error(&e))
        })
        .with_context(|| format!("DescribeStreamSummary {}", stream.display_name()))?;
    let s = resp
        .stream_description_summary
        .context("empty DescribeStreamSummary response")?;
    Ok(StreamInfo {
        name: s.stream_name,
        arn: s.stream_arn,
        status: s.stream_status.as_str().to_owned(),
        mode: s
            .stream_mode_details
            .map(|m| m.stream_mode.as_str().to_owned())
            .unwrap_or_else(|| "?".into()),
        open_shards: s.open_shard_count,
        retention_hours: s.retention_period_hours,
        max_record_kib: s.max_record_size_in_kib,
    })
}

#[derive(Clone)]
pub struct ReaderConfig {
    pub client: Client,
    pub stream: StreamRef,
    pub from: StartPos,
    pub poll: Duration,
    pub relist: Duration,
}

struct ShardInfo {
    id: String,
    closed: bool,
}

async fn list_shards(cfg: &ReaderConfig) -> Result<Vec<ShardInfo>, String> {
    let mut out = Vec::new();
    let mut token: Option<String> = None;
    loop {
        // With NextToken, the stream must not be named again.
        let req = match &token {
            Some(t) => cfg.client.list_shards().next_token(t),
            None => cfg
                .client
                .list_shards()
                .set_stream_name(cfg.stream.name.clone())
                .set_stream_arn(cfg.stream.arn.clone()),
        };
        let resp = req
            .send()
            .await
            .map_err(|e| format!("ListShards: {}", sdk_error(&e)))?;
        for s in resp.shards.unwrap_or_default() {
            let closed = s
                .sequence_number_range
                .as_ref()
                .is_some_and(|r| r.ending_sequence_number.is_some());
            out.push(ShardInfo {
                id: s.shard_id,
                closed,
            });
        }
        match resp.next_token {
            Some(t) => token = Some(t),
            None => return Ok(out),
        }
    }
}

/// Runs until the receiver goes away.
pub async fn run(cfg: ReaderConfig, tx: mpsc::Sender<SourceMsg>) {
    let (done_tx, mut done_rx) = mpsc::unbounded_channel::<Arc<str>>();
    let mut running: HashSet<Arc<str>> = HashSet::new();
    let mut finished: HashSet<Arc<str>> = HashSet::new();
    let mut initial = true;
    let mut backoff = Backoff::new(Duration::from_secs(1), Duration::from_secs(30));

    loop {
        match list_shards(&cfg).await {
            Ok(shards) => {
                backoff.reset();
                for s in shards {
                    let id: Arc<str> = s.id.into();
                    if running.contains(&id) || finished.contains(&id) {
                        continue;
                    }
                    let start = if initial {
                        match &cfg.from {
                            // Nothing new will ever land on a shard that's already closed.
                            StartPos::Latest if s.closed => {
                                finished.insert(id);
                                continue;
                            }
                            other => other.clone(),
                        }
                    } else {
                        // Appeared since we started (resharding): read all of it.
                        StartPos::TrimHorizon
                    };
                    running.insert(id.clone());
                    if tx.send(SourceMsg::ShardOpened(id.clone())).await.is_err() {
                        return;
                    }
                    let (cfg, tx, done) = (cfg.clone(), tx.clone(), done_tx.clone());
                    tokio::spawn(async move {
                        poll_shard(&cfg, id.clone(), start, &tx).await;
                        let _ = done.send(id);
                    });
                }
                initial = false;
            }
            Err(e) => {
                if tx.send(SourceMsg::Error(e)).await.is_err() {
                    return;
                }
                sleep(backoff.next()).await;
                continue;
            }
        }

        // Re-list on schedule, or as soon as a shard closes: that means resharding, so its
        // children should be picked up now rather than at the next scheduled re-list.
        tokio::select! {
            _ = sleep(cfg.relist) => {}
            Some(id) = done_rx.recv() => {
                running.remove(&id);
                finished.insert(id.clone());
                if tx.send(SourceMsg::ShardClosed(id)).await.is_err() {
                    return;
                }
                // Several shards usually close together; collect them before listing.
                while let Ok(id) = done_rx.try_recv() {
                    running.remove(&id);
                    finished.insert(id.clone());
                    if tx.send(SourceMsg::ShardClosed(id)).await.is_err() {
                        return;
                    }
                }
            }
        }
        if tx.is_closed() {
            return;
        }
    }
}

enum ErrorKind {
    Throttled,
    ExpiredIterator,
    Gone,
    BadIterator,
    Other,
}

fn classify<E: ProvideErrorMetadata, R>(e: &SdkError<E, R>) -> ErrorKind {
    match e.code() {
        Some(
            "ProvisionedThroughputExceededException"
            | "KMSThrottlingException"
            | "LimitExceededException"
            | "ThrottlingException",
        ) => ErrorKind::Throttled,
        Some("ExpiredIteratorException") => ErrorKind::ExpiredIterator,
        Some("ResourceNotFoundException") => ErrorKind::Gone,
        Some("InvalidArgumentException") => ErrorKind::BadIterator,
        _ => ErrorKind::Other,
    }
}

async fn get_iterator(
    cfg: &ReaderConfig,
    shard: &str,
    start: &StartPos,
    after_seq: Option<&str>,
) -> Result<
    Option<String>,
    SdkError<aws_sdk_kinesis::operation::get_shard_iterator::GetShardIteratorError>,
> {
    let mut req = cfg
        .client
        .get_shard_iterator()
        .set_stream_name(cfg.stream.name.clone())
        .set_stream_arn(cfg.stream.arn.clone())
        .shard_id(shard);
    req = match (after_seq, start) {
        (Some(seq), _) => req
            .shard_iterator_type(ShardIteratorType::AfterSequenceNumber)
            .starting_sequence_number(seq),
        (None, StartPos::Latest) => req.shard_iterator_type(ShardIteratorType::Latest),
        (None, StartPos::TrimHorizon) => req.shard_iterator_type(ShardIteratorType::TrimHorizon),
        (None, StartPos::AtTimestamp(ms)) => req
            .shard_iterator_type(ShardIteratorType::AtTimestamp)
            .timestamp(DateTime::from_millis(*ms)),
    };
    Ok(req.send().await?.shard_iterator)
}

fn arrival_ns(dt: &DateTime) -> i64 {
    i64::try_from(dt.as_nanos()).unwrap_or(i64::MAX)
}

/// Polls one shard until it's closed and drained (or gone, or the receiver hangs up).
async fn poll_shard(
    cfg: &ReaderConfig,
    shard: Arc<str>,
    mut start: StartPos,
    tx: &mpsc::Sender<SourceMsg>,
) {
    let mut iterator: Option<String> = None;
    let mut last_seq: Option<String> = None;
    let mut backoff = Backoff::new(Duration::from_millis(500), Duration::from_secs(10));

    loop {
        if iterator.is_none() {
            let requested_at = chrono::Utc::now().timestamp_millis();
            match get_iterator(cfg, &shard, &start, last_seq.as_deref()).await {
                Ok(Some(it)) => {
                    iterator = Some(it);
                    // If this iterator ever has to be replaced before reading a record, pick up
                    // from when it was taken rather than jumping to the new LATEST.
                    if start == StartPos::Latest {
                        start = StartPos::AtTimestamp(requested_at);
                    }
                }
                Ok(None) => return,
                Err(e) => {
                    let msg = match classify(&e) {
                        ErrorKind::Gone => return,
                        ErrorKind::Throttled => SourceMsg::Throttled,
                        _ => {
                            SourceMsg::Error(format!("GetShardIterator {shard}: {}", sdk_error(&e)))
                        }
                    };
                    if tx.send(msg).await.is_err() {
                        return;
                    }
                    sleep(backoff.next()).await;
                    continue;
                }
            }
        }

        let started = Instant::now();
        let Some(it) = iterator.clone() else { continue };
        // No SDK-level retries here: this loop already backs off, and it means throttling
        // shows up in the throttled count instead of as unexplained extra latency.
        let resp = cfg
            .client
            .get_records()
            .shard_iterator(it)
            .limit(GET_RECORDS_LIMIT)
            .customize()
            .config_override(
                aws_sdk_kinesis::config::Builder::default().retry_config(RetryConfig::disabled()),
            )
            .send()
            .await;
        match resp {
            Ok(resp) => {
                backoff.reset();
                if let Some(r) = resp.records.last() {
                    last_seq = Some(r.sequence_number.clone());
                }
                let pieces: Vec<Piece> = resp
                    .records
                    .into_iter()
                    .map(|r| Piece {
                        arrival_ns: r.approximate_arrival_timestamp.as_ref().map(arrival_ns),
                        data: r.data.into_inner(),
                        here_ns: None,
                    })
                    .collect();
                let batch = SourceMsg::Batch {
                    shard: shard.clone(),
                    pieces,
                    millis_behind: resp.millis_behind_latest,
                };
                if tx.send(batch).await.is_err() {
                    return;
                }
                match resp.next_shard_iterator {
                    Some(next) => iterator = Some(next),
                    // Closed (resharded) and fully read.
                    None => return,
                }
                sleep_until(started + cfg.poll).await;
            }
            Err(e) => {
                let msg = match classify(&e) {
                    ErrorKind::Gone => return,
                    ErrorKind::Throttled => SourceMsg::Throttled,
                    kind => {
                        if matches!(kind, ErrorKind::ExpiredIterator | ErrorKind::BadIterator) {
                            iterator = None;
                        }
                        SourceMsg::Error(format!("GetRecords {shard}: {}", sdk_error(&e)))
                    }
                };
                if tx.send(msg).await.is_err() {
                    return;
                }
                sleep(backoff.next()).await;
            }
        }
    }
}

/// Exponential backoff with jitter.
struct Backoff {
    base: Duration,
    max: Duration,
    attempt: u32,
}

impl Backoff {
    fn new(base: Duration, max: Duration) -> Backoff {
        Backoff {
            base,
            max,
            attempt: 0,
        }
    }

    fn reset(&mut self) {
        self.attempt = 0;
    }

    fn next(&mut self) -> Duration {
        let exp = self
            .base
            .saturating_mul(1u32 << self.attempt.min(16))
            .min(self.max);
        self.attempt += 1;
        // Up to 25% jitter, so throttled shards don't retry in lockstep.
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0);
        exp + exp.mul_f64(f64::from(nanos % 1000) / 4000.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_grows_and_caps() {
        let mut b = Backoff::new(Duration::from_millis(500), Duration::from_secs(10));
        let d: Vec<Duration> = (0..8).map(|_| b.next()).collect();
        assert!(d[0] >= Duration::from_millis(500) && d[0] <= Duration::from_millis(625));
        assert!(d[1] >= Duration::from_secs(1));
        assert!(d[7] >= Duration::from_secs(10) && d[7] <= Duration::from_millis(12_500));
        b.reset();
        assert!(b.next() <= Duration::from_millis(625));
    }

    #[test]
    fn arrival_precision() {
        let dt = DateTime::from_secs_and_nanos(1_705_318_200, 163_000_000);
        assert_eq!(arrival_ns(&dt), 1_705_318_200_163_000_000);
    }
}
