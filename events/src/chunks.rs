//! Reassembly of oversized CDC records.
//!
//! A `chunked` main record names one `chunk_id` per split image (`before`, `after` or both)
//! along with `total_fragments` and a CRC32C (decimal string) of the reassembled text.
//! `fragment` records carry `chunk_id`, a zero-based `index` and a slice of the image's JSON
//! text. Because CDC uses random partition keys, fragments can arrive before or after the main
//! record, on any shard, in any order, and more than once (at-least-once delivery; a
//! `chunk_id` is stable across retries).
//!
//! The reassembler is generic over the main record payload so the pipeline can carry its own
//! context and tests can use plain values. Time is passed in explicitly (ns) so expiry works the
//! same live and during replay.

use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::time::Duration;

use serde_json::value::RawValue;

use crate::record::ChunkRef;

/// Upper bound on buffered fragment text before the oldest orphans are dropped.
const DEFAULT_MAX_BUFFERED_BYTES: usize = 512 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Image {
    Before,
    After,
}

impl fmt::Display for Image {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Image::Before => "before",
            Image::After => "after",
        })
    }
}

#[derive(Debug, Default, Clone)]
pub struct ChunkStats {
    /// Main records fully reassembled and verified.
    pub reassembled: u64,
    /// Fragments consumed by those records.
    pub fragments_used: u64,
    pub duplicate_fragments: u64,
    pub duplicate_mains: u64,
    pub crc_errors: u64,
    /// Bad JSON after concatenation, zero fragments, or out-of-range indexes.
    pub invalid: u64,
    /// Reassembled without a usable `crc32c` to check against.
    pub unverified: u64,
    /// Main records given up on because fragments never arrived.
    pub expired_mains: u64,
    /// Fragments dropped because their main record never arrived.
    pub expired_fragments: u64,
}

#[derive(Debug)]
pub struct Assembled<T> {
    pub main: T,
    pub before: Option<Box<RawValue>>,
    pub after: Option<Box<RawValue>>,
    /// Number of fragments that went into this record.
    pub fragments: usize,
    /// Latest Kinesis arrival time among the main record and its fragments.
    pub max_arrival_ns: Option<i64>,
}

#[derive(Debug)]
pub enum Outcome<T> {
    Done(Assembled<T>),
    Failed { main: T, reason: String },
}

#[derive(Debug)]
pub struct Expired<T> {
    pub main: T,
    pub have: usize,
    pub want: usize,
}

struct Buf {
    parts: BTreeMap<u32, String>,
    first_seen_ns: i64,
    max_arrival_ns: Option<i64>,
    bytes: usize,
}

struct Pending<T> {
    main: T,
    needs: Vec<(Image, ChunkRef)>,
    first_seen_ns: i64,
    arrival_ns: Option<i64>,
}

pub struct Reassembler<T> {
    bufs: HashMap<String, Buf>,
    pending: HashMap<u64, Pending<T>>,
    /// chunk_id → pending main record waiting for it.
    waiting: HashMap<String, u64>,
    /// Recently completed chunk_ids, to recognize redelivered fragments and mains.
    completed: HashMap<String, i64>,
    next_id: u64,
    ttl_ns: i64,
    max_bytes: usize,
    bytes: usize,
    pub stats: ChunkStats,
}

impl<T> Reassembler<T> {
    pub fn new(ttl: Duration) -> Self {
        Reassembler {
            bufs: HashMap::new(),
            pending: HashMap::new(),
            waiting: HashMap::new(),
            completed: HashMap::new(),
            next_id: 0,
            ttl_ns: i64::try_from(ttl.as_nanos()).unwrap_or(i64::MAX),
            max_bytes: DEFAULT_MAX_BUFFERED_BYTES,
            bytes: 0,
            stats: ChunkStats::default(),
        }
    }

    #[cfg(test)]
    fn with_max_bytes(mut self, max: usize) -> Self {
        self.max_bytes = max;
        self
    }

    /// Main records waiting for fragments.
    pub fn pending(&self) -> usize {
        self.pending.len()
    }

    /// Fragments buffered (with or without a main record yet).
    pub fn buffered_fragments(&self) -> usize {
        self.bufs.values().map(|b| b.parts.len()).sum()
    }

    pub fn add_fragment(
        &mut self,
        chunk_id: &str,
        index: u32,
        data: String,
        arrival_ns: Option<i64>,
        now_ns: i64,
    ) -> Option<Outcome<T>> {
        if self.completed.contains_key(chunk_id) {
            self.stats.duplicate_fragments += 1;
            return None;
        }
        let buf = self.bufs.entry(chunk_id.to_owned()).or_insert_with(|| Buf {
            parts: BTreeMap::new(),
            first_seen_ns: now_ns,
            max_arrival_ns: None,
            bytes: 0,
        });
        if buf.parts.contains_key(&index) {
            self.stats.duplicate_fragments += 1;
            return None;
        }
        buf.bytes += data.len();
        self.bytes += data.len();
        buf.parts.insert(index, data);
        buf.max_arrival_ns = max_opt(buf.max_arrival_ns, arrival_ns);
        let id = *self.waiting.get(chunk_id)?;
        self.try_complete(id, now_ns)
    }

    /// Registers a main record. Returns the outcome right away if every fragment is already
    /// here, `None` if it's waiting (or is a duplicate of one already seen).
    pub fn add_main(
        &mut self,
        main: T,
        needs: Vec<(Image, ChunkRef)>,
        arrival_ns: Option<i64>,
        now_ns: i64,
    ) -> Option<Outcome<T>> {
        let seen = needs.iter().any(|(_, r)| {
            self.waiting.contains_key(&r.chunk_id) || self.completed.contains_key(&r.chunk_id)
        });
        if seen {
            self.stats.duplicate_mains += 1;
            return None;
        }
        let id = self.next_id;
        self.next_id += 1;
        for (_, r) in &needs {
            self.waiting.insert(r.chunk_id.clone(), id);
        }
        self.pending.insert(
            id,
            Pending {
                main,
                needs,
                first_seen_ns: now_ns,
                arrival_ns,
            },
        );
        self.try_complete(id, now_ns)
    }

    fn try_complete(&mut self, id: u64, now_ns: i64) -> Option<Outcome<T>> {
        let p = self.pending.get(&id)?;
        let ready = p.needs.iter().all(|(_, r)| {
            self.bufs
                .get(&r.chunk_id)
                .is_some_and(|b| (0..r.total_fragments).all(|i| b.parts.contains_key(&i)))
        });
        if !ready {
            return None;
        }
        let p = self.pending.remove(&id)?;
        let mut out = Assembled {
            main: p.main,
            before: None,
            after: None,
            fragments: 0,
            max_arrival_ns: p.arrival_ns,
        };
        let mut failure: Option<(String, bool)> = None;
        for (image, r) in &p.needs {
            self.waiting.remove(&r.chunk_id);
            self.completed.insert(r.chunk_id.clone(), now_ns);
            let Some(mut buf) = self.bufs.remove(&r.chunk_id) else {
                continue;
            };
            self.bytes -= buf.bytes;
            out.max_arrival_ns = max_opt(out.max_arrival_ns, buf.max_arrival_ns);
            if buf.parts.len() > r.total_fragments as usize {
                self.stats.invalid += (buf.parts.len() - r.total_fragments as usize) as u64;
            }
            let mut text = String::with_capacity(buf.bytes);
            for i in 0..r.total_fragments {
                if let Some(part) = buf.parts.remove(&i) {
                    text.push_str(&part);
                }
            }
            out.fragments += r.total_fragments as usize;
            if failure.is_some() {
                continue;
            }
            match verify_crc(&text, r.crc32c.as_deref()) {
                Ok(true) => {}
                Ok(false) => self.stats.unverified += 1,
                Err(e) => {
                    failure = Some((format!("{image} image {}: {e}", r.chunk_id), true));
                    continue;
                }
            }
            match RawValue::from_string(text) {
                Ok(raw) => match image {
                    Image::Before => out.before = Some(raw),
                    Image::After => out.after = Some(raw),
                },
                Err(e) => {
                    let reason = if r.total_fragments == 0 {
                        "total_fragments is 0".to_owned()
                    } else {
                        format!("reassembled text isn't valid JSON ({e})")
                    };
                    failure = Some((format!("{image} image {}: {reason}", r.chunk_id), false));
                }
            }
        }
        match failure {
            Some((reason, crc)) => {
                if crc {
                    self.stats.crc_errors += 1;
                } else {
                    self.stats.invalid += 1;
                }
                Some(Outcome::Failed {
                    main: out.main,
                    reason,
                })
            }
            None => {
                self.stats.reassembled += 1;
                self.stats.fragments_used += out.fragments as u64;
                Some(Outcome::Done(out))
            }
        }
    }

    /// Drops main records and orphan fragments older than the TTL. Returns the main records
    /// given up on so the caller can say so.
    pub fn expire(&mut self, now_ns: i64) -> Vec<Expired<T>> {
        let cutoff = now_ns.saturating_sub(self.ttl_ns);
        let mut out = Vec::new();

        let old: Vec<u64> = self
            .pending
            .iter()
            .filter(|(_, p)| p.first_seen_ns < cutoff)
            .map(|(id, _)| *id)
            .collect();
        for id in old {
            let Some(p) = self.pending.remove(&id) else {
                continue;
            };
            let (mut have, mut want) = (0, 0);
            for (_, r) in &p.needs {
                self.waiting.remove(&r.chunk_id);
                want += r.total_fragments as usize;
                if let Some(b) = self.bufs.remove(&r.chunk_id) {
                    self.bytes -= b.bytes;
                    have += b.parts.len();
                }
            }
            self.stats.expired_mains += 1;
            out.push(Expired {
                main: p.main,
                have,
                want,
            });
        }

        let stale: Vec<String> = self
            .bufs
            .iter()
            .filter(|(id, b)| b.first_seen_ns < cutoff && !self.waiting.contains_key(*id))
            .map(|(id, _)| id.clone())
            .collect();
        for id in stale {
            self.drop_buf(&id);
        }

        let keep_after = now_ns.saturating_sub(self.ttl_ns.saturating_mul(2));
        self.completed.retain(|_, t| *t >= keep_after);

        // Memory guard: shed the oldest orphan fragments first.
        while self.bytes > self.max_bytes {
            let oldest = self
                .bufs
                .iter()
                .filter(|(id, _)| !self.waiting.contains_key(*id))
                .min_by_key(|(_, b)| b.first_seen_ns)
                .map(|(id, _)| id.clone());
            match oldest {
                Some(id) => self.drop_buf(&id),
                None => break,
            }
        }
        out
    }

    fn drop_buf(&mut self, chunk_id: &str) {
        if let Some(b) = self.bufs.remove(chunk_id) {
            self.bytes -= b.bytes;
            self.stats.expired_fragments += b.parts.len() as u64;
        }
    }
}

/// `Ok(true)` verified, `Ok(false)` nothing usable to verify against, `Err` mismatch.
pub fn verify_crc(text: &str, expected: Option<&str>) -> Result<bool, String> {
    let Some(expected) = expected.and_then(|e| e.trim().parse::<u64>().ok()) else {
        return Ok(false);
    };
    let got = crc32c::crc32c(text.as_bytes());
    if u64::from(got) == expected {
        Ok(true)
    } else {
        Err(format!(
            "crc32c mismatch (record says {expected}, reassembled text is {got})"
        ))
    }
}

fn max_opt(a: Option<i64>, b: Option<i64>) -> Option<i64> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.max(b)),
        (a, b) => a.or(b),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const S: i64 = 1_000_000_000;
    const IMAGE: &str = r#"{"id":"0b6f0a43-6a2c-4f53-9a7e-2f1d8c9b1a11","title":"Héllo, wörld ✓","body":"a long body"}"#;

    fn crc(s: &str) -> String {
        crc32c::crc32c(s.as_bytes()).to_string()
    }

    /// Splits on char boundaries like DSQL does.
    fn split(s: &str, n: usize) -> Vec<String> {
        let chars: Vec<char> = s.chars().collect();
        let per = chars.len().div_ceil(n);
        chars.chunks(per).map(|c| c.iter().collect()).collect()
    }

    fn after_ref(id: &str, total: u32, crc32c: Option<String>) -> Vec<(Image, ChunkRef)> {
        vec![(
            Image::After,
            ChunkRef {
                chunk_id: id.into(),
                total_fragments: total,
                crc32c,
            },
        )]
    }

    fn done(o: Option<Outcome<&'static str>>) -> Assembled<&'static str> {
        match o {
            Some(Outcome::Done(a)) => a,
            Some(Outcome::Failed { reason, .. }) => panic!("failed: {reason}"),
            None => panic!("not complete"),
        }
    }

    #[test]
    fn main_first_then_fragments_in_order() {
        let mut r = Reassembler::new(Duration::from_secs(60));
        let parts = split(IMAGE, 3);
        assert!(
            r.add_main("m", after_ref("c1", 3, Some(crc(IMAGE))), Some(10), 0)
                .is_none()
        );
        assert!(
            r.add_fragment("c1", 0, parts[0].clone(), Some(11), 1)
                .is_none()
        );
        assert!(
            r.add_fragment("c1", 1, parts[1].clone(), Some(12), 2)
                .is_none()
        );
        let a = done(r.add_fragment("c1", 2, parts[2].clone(), Some(13), 3));
        assert_eq!(a.main, "m");
        assert_eq!(a.after.unwrap().get(), IMAGE);
        assert!(a.before.is_none());
        assert_eq!(a.fragments, 3);
        assert_eq!(a.max_arrival_ns, Some(13));
        assert_eq!(r.stats.reassembled, 1);
        assert_eq!(r.stats.fragments_used, 3);
        assert_eq!((r.pending(), r.buffered_fragments()), (0, 0));
    }

    #[test]
    fn fragments_out_of_order_before_main() {
        let mut r = Reassembler::new(Duration::from_secs(60));
        let parts = split(IMAGE, 4);
        assert!(
            r.add_fragment("c1", 3, parts[3].clone(), Some(40), 0)
                .is_none()
        );
        assert!(
            r.add_fragment("c1", 1, parts[1].clone(), Some(20), 0)
                .is_none()
        );
        assert!(r.add_fragment("c1", 0, parts[0].clone(), None, 0).is_none());
        assert!(
            r.add_fragment("c1", 2, parts[2].clone(), Some(30), 0)
                .is_none()
        );
        assert_eq!(r.buffered_fragments(), 4);
        let a = done(r.add_main("m", after_ref("c1", 4, Some(crc(IMAGE))), Some(5), 0));
        assert_eq!(a.after.unwrap().get(), IMAGE);
        assert_eq!(a.max_arrival_ns, Some(40));
    }

    #[test]
    fn main_in_the_middle() {
        let mut r = Reassembler::new(Duration::from_secs(60));
        let parts = split(IMAGE, 3);
        r.add_fragment("c1", 2, parts[2].clone(), None, 0);
        assert!(
            r.add_main("m", after_ref("c1", 3, Some(crc(IMAGE))), None, 0)
                .is_none()
        );
        r.add_fragment("c1", 0, parts[0].clone(), None, 0);
        let a = done(r.add_fragment("c1", 1, parts[1].clone(), None, 0));
        assert_eq!(a.after.unwrap().get(), IMAGE);
    }

    #[test]
    fn duplicate_fragments_and_mains() {
        let mut r = Reassembler::new(Duration::from_secs(60));
        let parts = split(IMAGE, 2);
        r.add_fragment("c1", 0, parts[0].clone(), None, 0);
        // Redelivered fragment: ignored, doesn't double the text.
        assert!(r.add_fragment("c1", 0, parts[0].clone(), None, 0).is_none());
        assert!(
            r.add_main("m", after_ref("c1", 2, Some(crc(IMAGE))), None, 0)
                .is_none()
        );
        // Redelivered main while still pending.
        assert!(
            r.add_main("m-dup", after_ref("c1", 2, Some(crc(IMAGE))), None, 0)
                .is_none()
        );
        let a = done(r.add_fragment("c1", 1, parts[1].clone(), None, 0));
        assert_eq!(a.main, "m");
        assert_eq!(a.after.unwrap().get(), IMAGE);
        // Late duplicates after completion are dropped, not buffered forever.
        assert!(r.add_fragment("c1", 1, parts[1].clone(), None, 1).is_none());
        assert!(
            r.add_main("m-late", after_ref("c1", 2, Some(crc(IMAGE))), None, 1)
                .is_none()
        );
        assert_eq!(r.buffered_fragments(), 0);
        assert_eq!(r.pending(), 0);
        assert_eq!(r.stats.duplicate_fragments, 2);
        assert_eq!(r.stats.duplicate_mains, 2);
        assert_eq!(r.stats.reassembled, 1);
    }

    #[test]
    fn crc_mismatch_fails() {
        let mut r = Reassembler::new(Duration::from_secs(60));
        let parts = split(IMAGE, 2);
        let wrong = (crc32c::crc32c(IMAGE.as_bytes()) ^ 1).to_string();
        r.add_main("m", after_ref("c1", 2, Some(wrong)), None, 0);
        r.add_fragment("c1", 1, parts[1].clone(), None, 0);
        match r.add_fragment("c1", 0, parts[0].clone(), None, 0) {
            Some(Outcome::Failed { main, reason }) => {
                assert_eq!(main, "m");
                assert!(reason.contains("crc32c mismatch"), "{reason}");
            }
            other => panic!("expected failure, got {other:?}"),
        }
        assert_eq!(r.stats.crc_errors, 1);
        assert_eq!(r.stats.reassembled, 0);
        assert_eq!((r.pending(), r.buffered_fragments()), (0, 0));
    }

    #[test]
    fn corrupted_fragment_text_is_caught_by_crc() {
        let mut r = Reassembler::new(Duration::from_secs(60));
        let parts = split(IMAGE, 2);
        r.add_main("m", after_ref("c1", 2, Some(crc(IMAGE))), None, 0);
        let corrupted = parts[0].replacen('0', "1", 1);
        assert_ne!(corrupted, parts[0]);
        r.add_fragment("c1", 0, corrupted, None, 0);
        assert!(matches!(
            r.add_fragment("c1", 1, parts[1].clone(), None, 0),
            Some(Outcome::Failed { .. })
        ));
    }

    #[test]
    fn crc_is_over_utf8_text_and_decimal() {
        // Known CRC32C test vector.
        assert_eq!(verify_crc("123456789", Some("3808858755")), Ok(true));
        assert_eq!(verify_crc("123456789", Some(" 3808858755 ")), Ok(true));
        assert!(verify_crc("123456789", Some("3808858756")).is_err());
        assert_eq!(verify_crc("x", None), Ok(false));
        assert_eq!(verify_crc("x", Some("0xdeadbeef")), Ok(false));
    }

    #[test]
    fn missing_crc_is_accepted_but_counted() {
        let mut r = Reassembler::new(Duration::from_secs(60));
        r.add_main("m", after_ref("c1", 1, None), None, 0);
        done(r.add_fragment("c1", 0, IMAGE.into(), None, 0));
        assert_eq!(r.stats.unverified, 1);
    }

    #[test]
    fn invalid_json_fails() {
        let mut r = Reassembler::new(Duration::from_secs(60));
        let text = r#"{"id": 1"#;
        r.add_main("m", after_ref("c1", 1, Some(crc(text))), None, 0);
        match r.add_fragment("c1", 0, text.into(), None, 0) {
            Some(Outcome::Failed { reason, .. }) => {
                assert!(reason.contains("valid JSON"), "{reason}")
            }
            other => panic!("expected failure, got {other:?}"),
        }
        assert_eq!(r.stats.invalid, 1);
    }

    #[test]
    fn before_and_after_both_chunked() {
        let mut r = Reassembler::new(Duration::from_secs(60));
        let before = r#"{"id":1}"#;
        let needs = vec![
            (
                Image::Before,
                ChunkRef {
                    chunk_id: "b".into(),
                    total_fragments: 1,
                    crc32c: Some(crc(before)),
                },
            ),
            (
                Image::After,
                ChunkRef {
                    chunk_id: "a".into(),
                    total_fragments: 2,
                    crc32c: Some(crc(IMAGE)),
                },
            ),
        ];
        let parts = split(IMAGE, 2);
        r.add_fragment("a", 1, parts[1].clone(), None, 0);
        assert!(r.add_main("m", needs, None, 0).is_none());
        assert!(
            r.add_fragment("b", 0, before.into(), None, 0).is_none(),
            "still waiting on after"
        );
        let a = done(r.add_fragment("a", 0, parts[0].clone(), None, 0));
        assert_eq!(a.before.unwrap().get(), before);
        assert_eq!(a.after.unwrap().get(), IMAGE);
        assert_eq!(a.fragments, 3);
    }

    #[test]
    fn interleaved_chunks_dont_mix() {
        let mut r = Reassembler::new(Duration::from_secs(60));
        let other = r#"{"id":2,"title":"other"}"#;
        let (p1, p2) = (split(IMAGE, 2), split(other, 2));
        r.add_main("one", after_ref("c1", 2, Some(crc(IMAGE))), None, 0);
        r.add_main("two", after_ref("c2", 2, Some(crc(other))), None, 0);
        r.add_fragment("c2", 1, p2[1].clone(), None, 0);
        r.add_fragment("c1", 0, p1[0].clone(), None, 0);
        let a = done(r.add_fragment("c2", 0, p2[0].clone(), None, 0));
        assert_eq!((a.main, a.after.unwrap().get()), ("two", other));
        let a = done(r.add_fragment("c1", 1, p1[1].clone(), None, 0));
        assert_eq!((a.main, a.after.unwrap().get()), ("one", IMAGE));
    }

    #[test]
    fn stale_partials_expire() {
        let mut r = Reassembler::new(Duration::from_secs(60));
        let parts = split(IMAGE, 3);
        r.add_main("m", after_ref("c1", 3, Some(crc(IMAGE))), None, 0);
        r.add_fragment("c1", 0, parts[0].clone(), None, S);
        // An orphan fragment whose main never shows up (e.g. written before we started).
        r.add_fragment("orphan", 4, "zzz".into(), None, 10 * S);
        assert!(r.expire(59 * S).is_empty());
        let gone = r.expire(61 * S);
        assert_eq!(gone.len(), 1);
        assert_eq!((gone[0].main, gone[0].have, gone[0].want), ("m", 1, 3));
        assert_eq!(r.pending(), 0);
        assert_eq!(r.buffered_fragments(), 1, "orphan is younger than the TTL");
        r.expire(71 * S);
        assert_eq!(r.buffered_fragments(), 0);
        assert_eq!(r.stats.expired_mains, 1);
        assert_eq!(r.stats.expired_fragments, 1);
        // Late fragment for the expired chunk just becomes a new orphan.
        assert!(
            r.add_fragment("c1", 1, parts[1].clone(), None, 72 * S)
                .is_none()
        );
    }

    #[test]
    fn byte_cap_sheds_oldest_orphans() {
        let mut r: Reassembler<&str> =
            Reassembler::new(Duration::from_secs(600)).with_max_bytes(10);
        r.add_fragment("old", 0, "123456".into(), None, 0);
        r.add_fragment("new", 0, "123456".into(), None, S);
        r.expire(2 * S);
        assert_eq!(r.buffered_fragments(), 1);
        assert!(
            r.add_fragment("new", 0, "123456".into(), None, 3 * S)
                .is_none(),
            "new survives"
        );
        assert_eq!(r.stats.duplicate_fragments, 1);
    }
}
