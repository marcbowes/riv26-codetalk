//! Raw Kinesis record bytes in, complete changes out.
//!
//! Both the live Kinesis reader and `replay` feed [`Piece`]s into the same [`Pipeline`], which
//! parses the envelope, reassembles chunked images, drops redeliveries, applies the table
//! filter and computes lag.

use std::collections::hash_map::DefaultHasher;
use std::collections::{HashSet, VecDeque};
use std::hash::{Hash, Hasher};
use std::time::Duration;

use serde_json::Value;
use serde_json::value::RawValue;

use crate::chunks::{ChunkStats, Outcome, Reassembler};
use crate::lag::Lag;
use crate::record::{Envelope, KNOWN_MAJOR_VERSION, Op, RecordType, major_version};

/// How many recent full records to remember for duplicate detection.
const DEDUP_CAPACITY: usize = 200_000;

/// One Kinesis record's worth of input.
#[derive(Debug, Clone)]
pub struct Piece {
    pub data: Vec<u8>,
    /// Kinesis `ApproximateArrivalTimestamp`, ns since the epoch.
    pub arrival_ns: Option<i64>,
    /// When this tool got the record, ns since the epoch. Live: stamped on processing. Replay:
    /// from the file, if it was recorded.
    pub here_ns: Option<i64>,
}

/// A complete change ready to display.
#[derive(Debug)]
pub struct Change {
    /// With chunked images already reassembled into `before`/`after`.
    pub env: Envelope,
    pub op: Op,
    pub lag: Lag,
    /// Commit time to display. Equal to the commit time live; shifted to "now" during replay
    /// so rehearsals look live while every lag stays as recorded.
    pub commit_display_ns: Option<i64>,
    /// Fragments reassembled into this change (0 for a full record).
    pub fragments: usize,
    /// The record as JSON, kept only for `--json`.
    pub raw: Option<Value>,
}

#[derive(Debug)]
pub enum Event {
    Change(Box<Change>),
    Warning(String),
}

#[derive(Debug, Default, Clone)]
pub struct Counters {
    pub records: u64,
    pub fragments_seen: u64,
    pub changes: u64,
    pub parse_errors: u64,
    pub unknown_types: u64,
    pub duplicates: u64,
    pub filtered: u64,
}

#[derive(Debug, Clone)]
pub struct PipelineOptions {
    pub tables: Vec<String>,
    pub keep_raw: bool,
    pub chunk_ttl: Duration,
}

struct MainCtx {
    env: Envelope,
    raw: Option<Value>,
}

struct Dedup {
    seen: HashSet<u64>,
    order: VecDeque<u64>,
    cap: usize,
}

impl Dedup {
    /// True if this key was seen recently.
    fn check(&mut self, key: u64) -> bool {
        if !self.seen.insert(key) {
            return true;
        }
        self.order.push_back(key);
        if self.order.len() > self.cap
            && let Some(old) = self.order.pop_front()
        {
            self.seen.remove(&old);
        }
        false
    }
}

/// A redelivered record has the same transaction, table, commit time and row image. CDC
/// processing time (`ts_ms`/`ts_ns`) can differ between deliveries, so it's not part of the key.
fn dedup_key(env: &Envelope) -> u64 {
    let mut h = DefaultHasher::new();
    let s = env.source.as_ref();
    s.and_then(|s| s.tx_id.as_deref()).hash(&mut h);
    s.and_then(|s| s.schema.as_deref()).hash(&mut h);
    s.and_then(|s| s.table.as_deref()).hash(&mut h);
    env.commit_ns().hash(&mut h);
    env.op.hash(&mut h);
    env.before.as_ref().map(|r| r.get()).hash(&mut h);
    env.after.as_ref().map(|r| r.get()).hash(&mut h);
    h.finish()
}

pub struct Pipeline {
    opts: PipelineOptions,
    reasm: Reassembler<MainCtx>,
    dedup: Dedup,
    warned: HashSet<String>,
    pub counters: Counters,
    /// Latest time seen in the input; drives chunk expiry during replay.
    clock_ns: Option<i64>,
}

impl Pipeline {
    pub fn new(opts: PipelineOptions) -> Self {
        Pipeline {
            reasm: Reassembler::new(opts.chunk_ttl),
            opts,
            dedup: Dedup {
                seen: HashSet::new(),
                order: VecDeque::new(),
                cap: DEDUP_CAPACITY,
            },
            warned: HashSet::new(),
            counters: Counters::default(),
            clock_ns: None,
        }
    }

    pub fn chunk_stats(&self) -> &ChunkStats {
        &self.reasm.stats
    }

    pub fn chunks_pending(&self) -> usize {
        self.reasm.pending()
    }

    pub fn fragments_buffered(&self) -> usize {
        self.reasm.buffered_fragments()
    }

    fn warn_once(&mut self, key: String, msg: String, out: &mut Vec<Event>) {
        if self.warned.insert(key) {
            out.push(Event::Warning(msg));
        }
    }

    pub fn push(&mut self, piece: Piece, wall_now_ns: i64, out: &mut Vec<Event>) {
        self.counters.records += 1;
        let env = match Envelope::parse(&piece.data) {
            Ok(env) => env,
            Err(e) => {
                self.counters.parse_errors += 1;
                // Log-scale rate limit: 1st, 2nd, 4th, 8th, …
                if self.counters.parse_errors.is_power_of_two() {
                    let preview =
                        crate::format::truncate(&String::from_utf8_lossy(&piece.data), 80);
                    out.push(Event::Warning(format!(
                        "{} record(s) aren't CDC JSON so far; latest: {e}: {preview}",
                        self.counters.parse_errors
                    )));
                }
                return;
            }
        };
        let reference = piece
            .here_ns
            .or(piece.arrival_ns)
            .or(env.cdc_ns())
            .or(env.commit_ns());
        if let Some(r) = reference {
            self.clock_ns = Some(self.clock_ns.map_or(r, |c| c.max(r)));
        }
        // A piece with no timestamps before any that have one (a bare fragment at the top of a
        // replay file) is treated as "now" so it isn't expired early.
        let now = self.clock_ns.unwrap_or(wall_now_ns);

        match env.record_type() {
            RecordType::Fragment => {
                self.counters.fragments_seen += 1;
                let Envelope {
                    chunk_id: Some(chunk_id),
                    index: Some(index),
                    data: Some(data),
                    ..
                } = env
                else {
                    self.warn_once(
                        "bad-fragment".into(),
                        "ignoring fragment record(s) without chunk_id/index/data".into(),
                        out,
                    );
                    return;
                };
                if let Some(o) =
                    self.reasm
                        .add_fragment(&chunk_id, index, data, piece.arrival_ns, now)
                {
                    self.finish_chunked(o, piece.here_ns, wall_now_ns, out);
                }
            }
            RecordType::Unknown(kind) => {
                self.counters.unknown_types += 1;
                self.warn_once(
                    format!("type:{kind}"),
                    format!(
                        "skipping record(s) with unknown type {kind:?} (newer envelope version?)"
                    ),
                    out,
                );
            }
            RecordType::Chunked => {
                self.check_version(&env, out);
                let refs = env.chunk_refs();
                let raw = self.raw(&piece.data);
                if refs.is_empty() {
                    // Nothing to wait for; show what's inline.
                    self.emit(
                        env,
                        raw,
                        piece.arrival_ns,
                        piece.here_ns,
                        0,
                        wall_now_ns,
                        out,
                    );
                    return;
                }
                let main = MainCtx { env, raw };
                if let Some(o) = self.reasm.add_main(main, refs, piece.arrival_ns, now) {
                    self.finish_chunked(o, piece.here_ns, wall_now_ns, out);
                }
            }
            RecordType::Full => {
                self.check_version(&env, out);
                if self.dedup.check(dedup_key(&env)) {
                    self.counters.duplicates += 1;
                    return;
                }
                let raw = self.raw(&piece.data);
                self.emit(
                    env,
                    raw,
                    piece.arrival_ns,
                    piece.here_ns,
                    0,
                    wall_now_ns,
                    out,
                );
            }
        }
    }

    /// Expires stale partial chunks. Live, `now_ns` is the wall clock; replay passes `None` to
    /// use the latest time seen in the file.
    pub fn tick(&mut self, now_ns: Option<i64>, out: &mut Vec<Event>) {
        if let Some(now) = now_ns.or(self.clock_ns) {
            self.expire(now, "after waiting", out);
        }
    }

    /// End of input: anything still partial is reported.
    pub fn finish(&mut self, out: &mut Vec<Event>) {
        self.expire(i64::MAX, "at end of input", out);
    }

    fn expire(&mut self, now: i64, when: &str, out: &mut Vec<Event>) {
        let orphans_before = self.reasm.stats.expired_fragments;
        for e in self.reasm.expire(now) {
            let env = &e.main.env;
            out.push(Event::Warning(format!(
                "gave up on chunked {} {} tx {} {when}: {}/{} fragments arrived",
                env.op().label(),
                env.table(),
                short_tx(env.tx_id()),
                e.have,
                e.want
            )));
        }
        let orphans = self.reasm.stats.expired_fragments - orphans_before;
        if orphans > 0 {
            self.warn_once(
                "orphans".into(),
                format!(
                    "dropped {orphans} fragment(s) whose main record never arrived \
                     (expected right after starting from latest)"
                ),
                out,
            );
        }
    }

    fn raw(&self, data: &[u8]) -> Option<Value> {
        if self.opts.keep_raw {
            serde_json::from_slice(data).ok()
        } else {
            None
        }
    }

    fn check_version(&mut self, env: &Envelope, out: &mut Vec<Event>) {
        let Some(v) = env.version() else { return };
        match major_version(v) {
            Some(KNOWN_MAJOR_VERSION) => {}
            major => {
                let key = major.map(|m| m.to_string()).unwrap_or_else(|| v.to_owned());
                self.warn_once(
                    format!("version:{key}"),
                    format!(
                        "CDC envelope version {v} is newer than this tool knows ({KNOWN_MAJOR_VERSION}.x); \
                         continuing and ignoring unknown fields"
                    ),
                    out,
                );
            }
        }
    }

    fn finish_chunked(
        &mut self,
        o: Outcome<MainCtx>,
        here: Option<i64>,
        wall_now_ns: i64,
        out: &mut Vec<Event>,
    ) {
        match o {
            Outcome::Done(a) => {
                let MainCtx { mut env, mut raw } = a.main;
                if let Some(obj) = raw.as_mut().and_then(Value::as_object_mut) {
                    for (key, img) in [("before", &a.before), ("after", &a.after)] {
                        if let Some(v) = img
                            .as_deref()
                            .and_then(|r| serde_json::from_str(r.get()).ok())
                        {
                            obj.insert(key.into(), v);
                        }
                    }
                }
                env.before = a.before.or(env.before);
                env.after = a.after.or(env.after);
                self.emit(
                    env,
                    raw,
                    a.max_arrival_ns,
                    here,
                    a.fragments,
                    wall_now_ns,
                    out,
                );
            }
            Outcome::Failed { main, reason } => {
                let env = &main.env;
                out.push(Event::Warning(format!(
                    "dropping chunked {} {} tx {}: {reason}",
                    env.op().label(),
                    env.table(),
                    short_tx(env.tx_id())
                )));
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn emit(
        &mut self,
        env: Envelope,
        raw: Option<Value>,
        arrival: Option<i64>,
        here: Option<i64>,
        fragments: usize,
        wall_now_ns: i64,
        out: &mut Vec<Event>,
    ) {
        if !self.table_matches(&env) {
            self.counters.filtered += 1;
            return;
        }
        let (commit, cdc) = (env.commit_ns(), env.cdc_ns());
        let lag = Lag::compute(commit, cdc, arrival, here);
        let reference = here.or(arrival).or(cdc).or(commit);
        let shift = reference
            .map(|r| wall_now_ns.saturating_sub(r))
            .unwrap_or(0);
        self.counters.changes += 1;
        out.push(Event::Change(Box::new(Change {
            op: env.op(),
            commit_display_ns: commit.map(|c| c.saturating_add(shift)),
            env,
            lag,
            fragments,
            raw,
        })));
    }

    fn table_matches(&self, env: &Envelope) -> bool {
        if self.opts.tables.is_empty() {
            return true;
        }
        let table = env.table();
        let qualified = env.schema().map(|s| format!("{s}.{table}"));
        self.opts.tables.iter().any(|t| {
            t.eq_ignore_ascii_case(table)
                || qualified
                    .as_deref()
                    .is_some_and(|q| t.eq_ignore_ascii_case(q))
        })
    }
}

/// First 8 characters of a base32 txId.
pub fn short_tx(tx: Option<&str>) -> String {
    match tx {
        Some(t) => t.chars().take(8).collect(),
        None => "?".into(),
    }
}

/// For `--json` and `--save`: the bytes as embedded JSON when they're valid, else a string.
pub fn raw_json(data: &[u8]) -> Box<RawValue> {
    let text = String::from_utf8_lossy(data).into_owned();
    RawValue::from_string(text.clone()).unwrap_or_else(|_| {
        RawValue::from_string(Value::String(text).to_string()).expect("a JSON string is valid JSON")
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::record::tests::{DOC_CHUNKED, DOC_DELETE, DOC_INSERT, DOC_UPDATE};
    use serde_json::json;

    const MS: i64 = 1_000_000;
    const SEC: i64 = 1_000_000_000;

    fn pipeline(keep_raw: bool, tables: &[&str]) -> Pipeline {
        Pipeline::new(PipelineOptions {
            tables: tables.iter().map(|s| s.to_string()).collect(),
            keep_raw,
            chunk_ttl: Duration::from_secs(60),
        })
    }

    fn piece(data: &str, arrival_ns: Option<i64>, here_ns: Option<i64>) -> Piece {
        Piece {
            data: data.as_bytes().to_vec(),
            arrival_ns,
            here_ns,
        }
    }

    fn changes(out: &[Event]) -> Vec<&Change> {
        out.iter()
            .filter_map(|e| match e {
                Event::Change(c) => Some(c.as_ref()),
                _ => None,
            })
            .collect()
    }

    fn warnings(out: &[Event]) -> Vec<&str> {
        out.iter()
            .filter_map(|e| match e {
                Event::Warning(w) => Some(w.as_str()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn doc_records_flow_through_with_lag() {
        let mut p = pipeline(false, &[]);
        let mut out = Vec::new();
        let commit = 1_705_318_200_000_000_000;
        let arrival = commit + 163 * MS;
        let here = commit + 411 * MS;
        p.push(piece(DOC_INSERT, Some(arrival), Some(here)), here, &mut out);
        p.push(piece(DOC_UPDATE, None, None), here, &mut out);
        p.push(piece(DOC_DELETE, None, None), here, &mut out);
        let cs = changes(&out);
        assert_eq!(cs.len(), 3);
        assert_eq!(
            [cs[0].op, cs[1].op, cs[2].op],
            [Op::Insert, Op::Update, Op::Delete]
        );
        let lag = cs[0].lag;
        assert_eq!(lag.commit_to_cdc, Some(125_483_291));
        assert_eq!(lag.cdc_to_kinesis, Some(163 * MS - 125_483_291));
        assert_eq!(lag.kinesis_to_here, Some(248 * MS));
        assert_eq!(lag.total, Some(411 * MS));
        // Live: here == wall clock, so the commit time is shown as-is.
        assert_eq!(cs[0].commit_display_ns, Some(commit));
        assert!(warnings(&out).is_empty());
        assert_eq!(p.counters.changes, 3);
    }

    #[test]
    fn replay_shifts_display_time_but_not_lag() {
        let mut p = pipeline(false, &[]);
        let mut out = Vec::new();
        let commit = 1_705_318_200_000_000_000;
        let here = commit + 500 * MS;
        let wall = here + 1_000 * SEC; // replaying much later
        p.push(
            piece(DOC_INSERT, Some(commit + 200 * MS), Some(here)),
            wall,
            &mut out,
        );
        let c = changes(&out)[0];
        assert_eq!(c.lag.total, Some(500 * MS));
        assert_eq!(c.commit_display_ns, Some(wall - 500 * MS));
    }

    #[test]
    fn duplicates_are_dropped_even_with_new_cdc_time() {
        let mut p = pipeline(false, &[]);
        let mut out = Vec::new();
        p.push(piece(DOC_INSERT, None, None), 0, &mut out);
        let redelivered = DOC_INSERT.replace("1705318200125483291", "1705318200999000000");
        p.push(piece(&redelivered, None, None), 0, &mut out);
        p.push(piece(DOC_UPDATE, None, None), 0, &mut out);
        assert_eq!(changes(&out).len(), 2);
        assert_eq!(p.counters.duplicates, 1);
    }

    #[test]
    fn chunked_record_reassembled_across_out_of_order_fragments() {
        let image = r#"{"order_id": 1001, "item_id": 42, "quantity": 5, "price": "29.99"}"#;
        let crc = crc32c::crc32c(image.as_bytes()).to_string();
        let main = DOC_CHUNKED.replace("2073618257", &crc);
        let (a, b, c) = (&image[..20], &image[20..40], &image[40..]);
        let frag = |i: u32, d: &str| {
            json!({"type": "fragment", "chunk_id": "chunk-id", "index": i, "data": d}).to_string()
        };

        let mut p = pipeline(true, &[]);
        let mut out = Vec::new();
        let t0 = 1_705_318_200_000_000_000;
        p.push(
            piece(&frag(2, c), Some(t0 + 150 * MS), Some(t0 + 400 * MS)),
            0,
            &mut out,
        );
        p.push(
            piece(&main, Some(t0 + 140 * MS), Some(t0 + 410 * MS)),
            0,
            &mut out,
        );
        p.push(
            piece(&frag(0, a), Some(t0 + 170 * MS), Some(t0 + 420 * MS)),
            0,
            &mut out,
        );
        assert!(out.is_empty());
        p.push(
            piece(&frag(1, b), Some(t0 + 160 * MS), Some(t0 + 450 * MS)),
            0,
            &mut out,
        );
        let cs = changes(&out);
        assert_eq!(cs.len(), 1);
        let ch = cs[0];
        assert_eq!(ch.fragments, 3);
        assert_eq!(ch.env.after.as_ref().unwrap().get(), image);
        assert_eq!(ch.env.after_value().unwrap()["quantity"], 5);
        // Arrival is the latest piece; "here" is when the last piece showed up.
        assert_eq!(ch.lag.cdc_to_kinesis, Some(170 * MS - 125_483_291));
        assert_eq!(ch.lag.total, Some(450 * MS));
        // --json output carries the reassembled image inline.
        let raw = ch.raw.as_ref().unwrap();
        assert_eq!(raw["after"]["price"], "29.99");
        assert_eq!(raw["type"], "chunked");
        assert_eq!(p.chunk_stats().reassembled, 1);
        assert_eq!(p.counters.fragments_seen, 3);
    }

    #[test]
    fn chunked_crc_mismatch_is_reported_not_shown() {
        let mut p = pipeline(false, &[]);
        let mut out = Vec::new();
        p.push(piece(DOC_CHUNKED, None, None), 0, &mut out);
        for (i, d) in [r#"{"order_id": 1"#, "001, \"item_id\"", ": 42}"]
            .iter()
            .enumerate()
        {
            let f = json!({"type": "fragment", "chunk_id": "chunk-id", "index": i, "data": d})
                .to_string();
            p.push(piece(&f, None, None), 0, &mut out);
        }
        assert!(changes(&out).is_empty());
        let w = warnings(&out);
        assert_eq!(w.len(), 1);
        assert!(
            w[0].contains("crc32c mismatch") && w[0].contains("order_items"),
            "{}",
            w[0]
        );
        assert_eq!(p.chunk_stats().crc_errors, 1);
    }

    #[test]
    fn partial_chunk_reported_at_end_of_replay() {
        let mut p = pipeline(false, &[]);
        let mut out = Vec::new();
        p.push(piece(DOC_CHUNKED, None, None), 0, &mut out);
        p.tick(None, &mut out);
        assert!(out.is_empty(), "within TTL");
        p.finish(&mut out);
        let w = warnings(&out);
        assert!(
            w[0].contains("gave up on chunked INSERT order_items tx ffthunp5"),
            "{}",
            w[0]
        );
        assert!(w[0].contains("0/3"), "{}", w[0]);
    }

    #[test]
    fn table_filter() {
        let mut p = pipeline(false, &["public.comments", "ORDER_ITEMS"]);
        let mut out = Vec::new();
        p.push(piece(DOC_INSERT, None, None), 0, &mut out);
        p.push(
            piece(&DOC_UPDATE.replace("order_items", "posts"), None, None),
            0,
            &mut out,
        );
        p.push(
            piece(&DOC_DELETE.replace("order_items", "comments"), None, None),
            0,
            &mut out,
        );
        let tables: Vec<&str> = changes(&out).iter().map(|c| c.env.table()).collect();
        assert_eq!(tables, ["order_items", "comments"]);
        assert_eq!(p.counters.filtered, 1);
    }

    #[test]
    fn warns_once_per_unknown_major_version_and_type() {
        let mut p = pipeline(false, &[]);
        let mut out = Vec::new();
        let v2 = DOC_INSERT.replace("\"1.0\"", "\"2.1\"");
        let v2b = DOC_UPDATE.replace("\"1.0\"", "\"2.3\"");
        let v1_9 = DOC_DELETE.replace("\"1.0\"", "\"1.9\"");
        for d in [&v2, &v2b, &v1_9] {
            p.push(piece(d, None, None), 0, &mut out);
        }
        p.push(piece(r#"{"type":"heartbeat"}"#, None, None), 0, &mut out);
        p.push(piece(r#"{"type":"heartbeat"}"#, None, None), 0, &mut out);
        let w = warnings(&out);
        assert_eq!(w.len(), 2, "{w:?}");
        assert!(w[0].contains("2.1"));
        assert!(w[1].contains("heartbeat"));
        // Records with a newer minor or major version are still shown.
        assert_eq!(changes(&out).len(), 3);
        assert_eq!(p.counters.unknown_types, 2);
    }

    #[test]
    fn garbage_is_counted_and_rate_limited() {
        let mut p = pipeline(false, &[]);
        let mut out = Vec::new();
        for _ in 0..5 {
            p.push(piece("not json", None, None), 0, &mut out);
        }
        assert_eq!(p.counters.parse_errors, 5);
        assert_eq!(warnings(&out).len(), 3, "1st, 2nd and 4th");
    }

    #[test]
    fn raw_json_embeds_or_quotes() {
        assert_eq!(raw_json(br#"{"a":1}"#).get(), r#"{"a":1}"#);
        assert_eq!(raw_json(b"oops").get(), r#""oops""#);
    }
}
