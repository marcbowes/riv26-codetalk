//! The Aurora DSQL CDC record envelope.
//!
//! Field names and meanings follow "Understanding CDC records" in the Aurora DSQL user guide.
//! Every field is optional and loosely typed so that unknown fields, new enum values and minor
//! version bumps don't break parsing. Row images are kept as raw JSON text: parsing them into
//! a tree is only needed when a row is displayed, which matters under heavy load.

use serde::{Deserialize, Deserializer};
use serde_json::Value;
use serde_json::value::RawValue;

/// The envelope major version this tool understands.
pub const KNOWN_MAJOR_VERSION: u64 = 1;

/// The top-level `type` field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecordType {
    /// A complete record with inline `before` and `after`.
    Full,
    /// A main record whose `before` and/or `after` image is split into fragments.
    Chunked,
    /// One piece of a chunked image.
    Fragment,
    /// A `type` this tool doesn't know about (a newer minor version).
    Unknown(String),
}

/// The `op` field. Unknown values map to `Other` so they can still be counted and shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Op {
    Insert,
    Update,
    Delete,
    Other,
}

impl Op {
    pub fn parse(s: Option<&str>) -> Op {
        match s {
            Some("c") => Op::Insert,
            Some("u") => Op::Update,
            Some("d") => Op::Delete,
            _ => Op::Other,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Op::Insert => "INSERT",
            Op::Update => "UPDATE",
            Op::Delete => "DELETE",
            Op::Other => "OTHER",
        }
    }
}

#[derive(Debug, Default, Deserialize)]
pub struct Envelope {
    #[serde(rename = "type", default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub op: Option<String>,
    /// Primary key of a deleted row; JSON `null` and absent both become `None`.
    #[serde(default)]
    pub before: Option<Box<RawValue>>,
    /// Full row after an insert or update.
    #[serde(default)]
    pub after: Option<Box<RawValue>>,
    #[serde(default)]
    pub source: Option<Source>,
    /// Reassembly metadata on a `chunked` main record.
    #[serde(default)]
    pub chunked: Option<ChunkedRefs>,
    /// When the CDC system processed the record.
    #[serde(default, deserialize_with = "lenient_i64")]
    pub ts_ms: Option<i64>,
    #[serde(default, deserialize_with = "lenient_i64")]
    pub ts_ns: Option<i64>,

    // Fragment records only.
    #[serde(default)]
    pub chunk_id: Option<String>,
    #[serde(default, deserialize_with = "lenient_u32")]
    pub index: Option<u32>,
    #[serde(default)]
    pub data: Option<String>,
}

#[derive(Debug, Default, Clone, Deserialize)]
pub struct Source {
    #[serde(default, deserialize_with = "lenient_string")]
    pub version: Option<String>,
    /// Transaction commit time.
    #[serde(default, deserialize_with = "lenient_i64")]
    pub ts_ms: Option<i64>,
    #[serde(default, deserialize_with = "lenient_i64")]
    pub ts_ns: Option<i64>,
    #[serde(default, rename = "txId")]
    pub tx_id: Option<String>,
    #[serde(default)]
    pub schema: Option<String>,
    #[serde(default)]
    pub table: Option<String>,
    #[serde(default)]
    pub cluster: Option<String>,
}

#[derive(Debug, Default, Clone, Deserialize)]
pub struct ChunkedRefs {
    #[serde(default)]
    pub before: Option<ChunkRef>,
    #[serde(default)]
    pub after: Option<ChunkRef>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct ChunkRef {
    pub chunk_id: String,
    #[serde(deserialize_with = "required_u32")]
    pub total_fragments: u32,
    /// CRC32C of the reassembled image text, as a decimal string.
    #[serde(default, deserialize_with = "lenient_string")]
    pub crc32c: Option<String>,
}

impl Envelope {
    pub fn parse(data: &[u8]) -> serde_json::Result<Envelope> {
        serde_json::from_slice(data)
    }

    pub fn record_type(&self) -> RecordType {
        match self.kind.as_deref() {
            Some("full") => RecordType::Full,
            Some("chunked") => RecordType::Chunked,
            Some("fragment") => RecordType::Fragment,
            Some(other) => RecordType::Unknown(other.to_owned()),
            // No `type` at all: infer from shape rather than drop the record.
            None if self.chunk_id.is_some() && self.data.is_some() => RecordType::Fragment,
            None if self.chunked.is_some() => RecordType::Chunked,
            None => RecordType::Full,
        }
    }

    pub fn op(&self) -> Op {
        Op::parse(self.op.as_deref())
    }

    /// Transaction commit time in ns, preferring `source.ts_ns`.
    pub fn commit_ns(&self) -> Option<i64> {
        let s = self.source.as_ref()?;
        s.ts_ns.or(s.ts_ms.map(ms_to_ns))
    }

    /// CDC processing time in ns, preferring `ts_ns`.
    pub fn cdc_ns(&self) -> Option<i64> {
        self.ts_ns.or(self.ts_ms.map(ms_to_ns))
    }

    pub fn table(&self) -> &str {
        self.source
            .as_ref()
            .and_then(|s| s.table.as_deref())
            .unwrap_or("?")
    }

    pub fn schema(&self) -> Option<&str> {
        self.source.as_ref().and_then(|s| s.schema.as_deref())
    }

    pub fn tx_id(&self) -> Option<&str> {
        self.source.as_ref().and_then(|s| s.tx_id.as_deref())
    }

    pub fn cluster(&self) -> Option<&str> {
        self.source.as_ref().and_then(|s| s.cluster.as_deref())
    }

    pub fn version(&self) -> Option<&str> {
        self.source.as_ref().and_then(|s| s.version.as_deref())
    }

    /// The chunked images a main record is waiting for.
    pub fn chunk_refs(&self) -> Vec<(crate::chunks::Image, ChunkRef)> {
        use crate::chunks::Image;
        let mut refs = Vec::new();
        if let Some(c) = &self.chunked {
            if let Some(r) = &c.before {
                refs.push((Image::Before, r.clone()));
            }
            if let Some(r) = &c.after {
                refs.push((Image::After, r.clone()));
            }
        }
        refs
    }

    pub fn before_value(&self) -> Option<Value> {
        self.before
            .as_ref()
            .and_then(|r| serde_json::from_str(r.get()).ok())
    }

    pub fn after_value(&self) -> Option<Value> {
        self.after
            .as_ref()
            .and_then(|r| serde_json::from_str(r.get()).ok())
    }
}

/// `"1.0"` → 1. `None` if it isn't a dotted number.
pub fn major_version(v: &str) -> Option<u64> {
    v.trim().split('.').next()?.trim().parse().ok()
}

pub fn ms_to_ns(ms: i64) -> i64 {
    ms.saturating_mul(1_000_000)
}

fn value_to_i64(v: &Value) -> Option<i64> {
    match v {
        Value::Number(n) => n
            .as_i64()
            .or_else(|| n.as_u64().map(|u| i64::try_from(u).unwrap_or(i64::MAX)))
            .or_else(|| n.as_f64().map(|f| f as i64)),
        Value::String(s) => {
            let s = s.trim();
            s.parse::<i64>()
                .ok()
                .or_else(|| s.parse::<f64>().ok().map(|f| f as i64))
        }
        _ => None,
    }
}

/// Accepts an integer, a float or a numeric string. Anything else becomes `None`.
fn lenient_i64<'de, D: Deserializer<'de>>(d: D) -> Result<Option<i64>, D::Error> {
    Ok(Option::<Value>::deserialize(d)?
        .as_ref()
        .and_then(value_to_i64))
}

fn lenient_u32<'de, D: Deserializer<'de>>(d: D) -> Result<Option<u32>, D::Error> {
    Ok(Option::<Value>::deserialize(d)?
        .as_ref()
        .and_then(value_to_i64)
        .and_then(|n| u32::try_from(n).ok()))
}

fn required_u32<'de, D: Deserializer<'de>>(d: D) -> Result<u32, D::Error> {
    let v = Value::deserialize(d)?;
    value_to_i64(&v)
        .and_then(|n| u32::try_from(n).ok())
        .ok_or_else(|| serde::de::Error::custom(format!("expected a fragment count, got {v}")))
}

/// Accepts a string or a number (kept as its JSON text).
fn lenient_string<'de, D: Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    Ok(match Option::<Value>::deserialize(d)? {
        Some(Value::String(s)) => Some(s),
        Some(Value::Number(n)) => Some(n.to_string()),
        _ => None,
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::chunks::Image;

    // The examples from "Understanding CDC records", verbatim.
    pub const DOC_INSERT: &str = r#"{
        "type": "full",
        "op": "c",
        "before": null,
        "after": {"order_id": 1001, "item_id": 42, "quantity": 5, "price": "29.99"},
        "source": {
            "version": "1.0",
            "ts_ms": 1705318200000,
            "ts_ns": 1705318200000000000,
            "txId": "ffthunp5stx6ffs2vyfqoatmfu",
            "schema": "public",
            "table": "order_items",
            "db": "postgres",
            "cluster": "kmabugltfmjdaj2siqr2qbxgju"
        },
        "ts_ms": 1705318200125,
        "ts_ns": 1705318200125483291
    }"#;

    pub const DOC_UPDATE: &str = r#"{
        "type": "full",
        "op": "u",
        "before": null,
        "after": {"order_id": 1001, "item_id": 42, "quantity": 10, "price": "29.99"},
        "source": {
            "version": "1.0",
            "ts_ms": 1705318300000,
            "ts_ns": 1705318300000000000,
            "txId": "qvtiesgmd55cvlfukm3dfuotji",
            "schema": "public",
            "table": "order_items",
            "db": "postgres",
            "cluster": "kmabugltfmjdaj2siqr2qbxgju"
        },
        "ts_ms": 1705318300125,
        "ts_ns": 1705318300125483291
    }"#;

    pub const DOC_DELETE: &str = r#"{
        "type": "full",
        "op": "d",
        "before": {"order_id": 1001, "item_id": 42},
        "after": null,
        "source": {
            "version": "1.0",
            "ts_ms": 1705318400000,
            "ts_ns": 1705318400000000000,
            "txId": "xyzabc123def456ghi789jklmno",
            "schema": "public",
            "table": "order_items",
            "db": "postgres",
            "cluster": "kmabugltfmjdaj2siqr2qbxgju"
        },
        "ts_ms": 1705318400125,
        "ts_ns": 1705318400125483291
    }"#;

    pub const DOC_CHUNKED: &str = r#"{
        "type": "chunked",
        "op": "c",
        "before": null,
        "after": null,
        "source": {
            "version": "1.0",
            "ts_ms": 1705318200000,
            "ts_ns": 1705318200000000000,
            "txId": "ffthunp5stx6ffs2vyfqoatmfu",
            "schema": "public",
            "table": "order_items",
            "db": "postgres",
            "cluster": "cluster-id"
        },
        "chunked": {
            "after": {
                "chunk_id": "chunk-id",
                "total_fragments": 3,
                "crc32c": "2073618257"
            }
        },
        "ts_ms": 1705318200125,
        "ts_ns": 1705318200125483291
    }"#;

    pub const DOC_FRAGMENT: &str = r#"{
        "type": "fragment",
        "chunk_id": "chunk-id",
        "index": 0,
        "data": "partial-JSON-text"
    }"#;

    fn parse(s: &str) -> Envelope {
        Envelope::parse(s.as_bytes()).expect("parses")
    }

    #[test]
    fn doc_insert() {
        let e = parse(DOC_INSERT);
        assert_eq!(e.record_type(), RecordType::Full);
        assert_eq!(e.op(), Op::Insert);
        assert!(e.before.is_none(), "null before becomes None");
        let after = e.after_value().unwrap();
        assert_eq!(after["order_id"], 1001);
        assert_eq!(after["price"], "29.99");
        assert_eq!(e.table(), "order_items");
        assert_eq!(e.schema(), Some("public"));
        assert_eq!(e.tx_id(), Some("ffthunp5stx6ffs2vyfqoatmfu"));
        assert_eq!(e.version(), Some("1.0"));
        assert_eq!(major_version(e.version().unwrap()), Some(1));
        // ns fields are larger than 2^53 and must survive exactly.
        assert_eq!(e.commit_ns(), Some(1_705_318_200_000_000_000));
        assert_eq!(e.cdc_ns(), Some(1_705_318_200_125_483_291));
        assert_eq!(e.cluster(), Some("kmabugltfmjdaj2siqr2qbxgju"));
        assert!(e.chunk_refs().is_empty());
    }

    #[test]
    fn doc_update() {
        let e = parse(DOC_UPDATE);
        assert_eq!(e.record_type(), RecordType::Full);
        assert_eq!(e.op(), Op::Update);
        assert_eq!(e.after_value().unwrap()["quantity"], 10);
        assert!(e.before.is_none());
    }

    #[test]
    fn doc_delete() {
        let e = parse(DOC_DELETE);
        assert_eq!(e.op(), Op::Delete);
        assert!(e.after.is_none());
        let before = e.before_value().unwrap();
        assert_eq!(before, serde_json::json!({"order_id": 1001, "item_id": 42}));
    }

    #[test]
    fn doc_chunked_main() {
        let e = parse(DOC_CHUNKED);
        assert_eq!(e.record_type(), RecordType::Chunked);
        assert_eq!(e.op(), Op::Insert);
        assert!(e.after.is_none());
        let refs = e.chunk_refs();
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].0, Image::After);
        assert_eq!(refs[0].1.chunk_id, "chunk-id");
        assert_eq!(refs[0].1.total_fragments, 3);
        assert_eq!(refs[0].1.crc32c.as_deref(), Some("2073618257"));
    }

    #[test]
    fn doc_fragment() {
        let e = parse(DOC_FRAGMENT);
        assert_eq!(e.record_type(), RecordType::Fragment);
        assert_eq!(e.chunk_id.as_deref(), Some("chunk-id"));
        assert_eq!(e.index, Some(0));
        assert_eq!(e.data.as_deref(), Some("partial-JSON-text"));
        assert!(e.source.is_none());
    }

    #[test]
    fn tolerates_unknown_fields_and_enum_values() {
        let e = parse(
            r#"{"type":"full","op":"t","after":{"a":1},"new_top":{"x":[1,2]},
                "source":{"version":"1.7","ts_ms":1,"txId":"t","table":"x","shiny":true}}"#,
        );
        assert_eq!(e.record_type(), RecordType::Full);
        assert_eq!(e.op(), Op::Other);
        assert_eq!(e.op.as_deref(), Some("t"));
        assert_eq!(major_version(e.version().unwrap()), Some(1));

        let e = parse(r#"{"type":"heartbeat","source":{"version":"2.0"}}"#);
        assert_eq!(e.record_type(), RecordType::Unknown("heartbeat".into()));
        assert_eq!(major_version(e.version().unwrap()), Some(2));
    }

    #[test]
    fn lenient_numbers() {
        let e = parse(
            r#"{"type":"full","op":"c","ts_ns":"1705318200125483291","ts_ms":1705318200125.0,
                "source":{"version":1.0,"ts_ms":"1705318200000"}}"#,
        );
        assert_eq!(e.cdc_ns(), Some(1_705_318_200_125_483_291));
        assert_eq!(e.ts_ms, Some(1_705_318_200_125));
        // No source.ts_ns: fall back to ms.
        assert_eq!(e.commit_ns(), Some(1_705_318_200_000_000_000));
        assert_eq!(e.version(), Some("1.0"));
    }

    #[test]
    fn missing_type_is_inferred() {
        assert_eq!(
            parse(r#"{"chunk_id":"c","index":1,"data":"x"}"#).record_type(),
            RecordType::Fragment
        );
        assert_eq!(
            parse(r#"{"op":"c","chunked":{"after":{"chunk_id":"c","total_fragments":2}}}"#)
                .record_type(),
            RecordType::Chunked
        );
        assert_eq!(
            parse(r#"{"op":"c","after":{}}"#).record_type(),
            RecordType::Full
        );
    }

    #[test]
    fn both_images_chunked() {
        let e = parse(
            r#"{"type":"chunked","op":"u","chunked":{
                "before":{"chunk_id":"b","total_fragments":"2","crc32c":"1"},
                "after":{"chunk_id":"a","total_fragments":4,"crc32c":12345}}}"#,
        );
        let refs = e.chunk_refs();
        assert_eq!(refs.len(), 2);
        assert_eq!((refs[0].0, refs[0].1.total_fragments), (Image::Before, 2));
        assert_eq!(
            (refs[1].0, refs[1].1.crc32c.as_deref()),
            (Image::After, Some("12345"))
        );
    }
}
