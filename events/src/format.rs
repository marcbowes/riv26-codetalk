//! Turning row images into a short, readable summary.
//!
//! CDC value encodings (from the docs): timestamptz is a JSON number of microseconds since the
//! epoch, uuid is a string, numeric is a string, and json/jsonb is a JSON *string* containing the
//! raw JSON text. Column types aren't in the record, so those are recognized by name (`*_at`) or
//! by content.

use chrono::{DateTime, FixedOffset, Local, NaiveDate, NaiveDateTime, Utc};
use serde_json::{Map, Value};

use crate::record::Op;

/// ±infinity sentinels for timestamp/timestamptz.
const TS_PLUS_INFINITY: i64 = 9_223_372_036_825_200_000;
const TS_MINUS_INFINITY: i64 = -9_223_372_036_832_400_000;

const TEXT_MAX: usize = 48;
const JSON_MAX: usize = 60;
const GENERIC_MAX: usize = 240;

/// Where and when "today" is, for formatting timestamps compactly.
#[derive(Debug, Clone, Copy)]
pub struct TimeCtx {
    /// `None` means the local time zone (with DST rules); tests pin an offset.
    pub offset: Option<FixedOffset>,
    pub today: NaiveDate,
}

impl TimeCtx {
    pub fn local() -> TimeCtx {
        TimeCtx {
            offset: None,
            today: Local::now().date_naive(),
        }
    }

    pub fn naive(&self, dt: DateTime<Utc>) -> NaiveDateTime {
        match self.offset {
            Some(o) => dt.with_timezone(&o).naive_local(),
            None => dt.with_timezone(&Local).naive_local(),
        }
    }
}

/// A timestamp(tz) column value: today's times as `14:02:11.482`, other days with the date.
pub fn fmt_micros(us: i64, t: &TimeCtx) -> String {
    match us {
        TS_PLUS_INFINITY => return "infinity".into(),
        TS_MINUS_INFINITY => return "-infinity".into(),
        _ => {}
    }
    match DateTime::from_timestamp_micros(us) {
        Some(dt) => {
            let local = t.naive(dt);
            if local.date() == t.today {
                local.format("%H:%M:%S%.3f").to_string()
            } else {
                local.format("%Y-%m-%d %H:%M:%S").to_string()
            }
        }
        None => us.to_string(),
    }
}

/// Truncates to `max` characters, ending in `…` when cut.
pub fn truncate(s: &str, max: usize) -> String {
    if max == 0 {
        return String::new();
    }
    if s.chars().count() <= max {
        return s.to_owned();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// Quoted, single-line, truncated text.
pub fn quote(s: &str, max: usize) -> String {
    let flat: String = s
        .chars()
        .filter(|c| *c != '\r')
        .map(|c| match c {
            '\n' => '↵',
            c if c.is_control() => ' ',
            c => c,
        })
        .collect();
    format!("\"{}\"", truncate(&flat, max))
}

pub fn is_uuid(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 36
        && b.iter().enumerate().all(|(i, c)| match i {
            8 | 13 | 18 | 23 => *c == b'-',
            _ => c.is_ascii_hexdigit(),
        })
}

/// First 8 hex digits of a uuid, which is plenty to tell rows apart on screen.
pub fn short_uuid(s: &str) -> Option<String> {
    is_uuid(s).then(|| format!("{}…", &s[..8]))
}

/// If `s` is JSON text for an object or array (how json/jsonb columns arrive), parse it.
pub fn embedded_json(s: &str) -> Option<Value> {
    let t = s.trim_start();
    if !(t.starts_with('{') || t.starts_with('[')) {
        return None;
    }
    serde_json::from_str::<Value>(s)
        .ok()
        .filter(|v| v.is_object() || v.is_array())
}

fn looks_numeric(s: &str) -> bool {
    matches!(s, "NaN" | "Infinity" | "-Infinity")
        || (!s.is_empty()
            && s.len() <= 40
            && s.parse::<f64>().is_ok()
            && s.bytes()
                .all(|b| b.is_ascii_digit() || matches!(b, b'-' | b'+' | b'.' | b'e' | b'E')))
}

/// One column value, compactly.
pub fn fmt_value(key: &str, v: &Value, t: &TimeCtx) -> String {
    match v {
        Value::Null => "null".into(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => match n.as_i64() {
            Some(us) if key.ends_with("_at") => fmt_micros(us, t),
            _ => n.to_string(),
        },
        Value::String(s) => {
            if let Some(short) = short_uuid(s) {
                short
            } else if let Some(j) = embedded_json(s) {
                truncate(&j.to_string(), JSON_MAX)
            } else if looks_numeric(s) {
                s.clone()
            } else {
                quote(s, TEXT_MAX)
            }
        }
        other => truncate(&other.to_string(), JSON_MAX),
    }
}

fn id(v: Option<&Value>, t: &TimeCtx) -> Option<String> {
    v.filter(|v| !v.is_null()).map(|v| fmt_value("", v, t))
}

pub fn fmt_bytes(n: usize) -> String {
    match n {
        n if n < 1024 => format!("{n} B"),
        n if n < 1024 * 1024 => format!("{:.1} KB", n as f64 / 1024.0),
        n => format!("{:.1} MB", n as f64 / (1024.0 * 1024.0)),
    }
}

/// `col=value col=value …` in column order.
pub fn generic(map: &Map<String, Value>, t: &TimeCtx) -> String {
    let parts: Vec<String> = map
        .iter()
        .map(|(k, v)| format!("{k}={}", fmt_value(k, v, t)))
        .collect();
    truncate(&parts.join(" "), GENERIC_MAX)
}

fn comment(after: &Map<String, Value>, t: &TimeCtx) -> Option<String> {
    let mut s = quote(after.get("body")?.as_str()?, TEXT_MAX);
    if let Some(u) = id(after.get("user_id"), t) {
        s += &format!(" by user {u}");
    }
    if let Some(p) = id(after.get("post_id"), t) {
        s += &format!(" on post {p}");
    }
    Some(s)
}

fn post(after: &Map<String, Value>, t: &TimeCtx) -> Option<String> {
    let mut s = quote(after.get("title")?.as_str()?, TEXT_MAX);
    if let Some(a) = id(after.get("author_id"), t) {
        s += &format!(" by {a}");
    }
    let meta = match after.get("metadata") {
        Some(Value::String(text)) => embedded_json(text),
        Some(v @ Value::Object(_)) => Some(v.clone()),
        _ => None,
    };
    if let Some(meta) = meta {
        if let Some(c) = meta.get("category").and_then(Value::as_str) {
            s += &format!(" [{c}]");
        }
        if let Some(tags) = meta.get("tags").and_then(Value::as_array) {
            for tag in tags.iter().filter_map(Value::as_str).take(3) {
                s += &format!(" #{tag}");
            }
        }
        if let Some(m) = meta.get("readingMinutes").and_then(Value::as_f64) {
            s += &format!(" · {m} min");
        }
    }
    if let Some(body) = after.get("body").and_then(Value::as_str) {
        s += &format!(" · body {}", fmt_bytes(body.len()));
    }
    Some(s)
}

fn user(after: &Map<String, Value>) -> Option<String> {
    let mut s = format!("@{}", after.get("username")?.as_str()?);
    if let Some(d) = after.get("display_name").and_then(Value::as_str) {
        s += &format!(" {}", quote(d, TEXT_MAX));
    }
    Some(s)
}

/// The row part of a tail line. Deletes show the primary key from `before`; inserts and
/// updates show a few interesting columns from `after` (templates for the blog tables, every
/// column for anything else).
pub fn row_summary(
    table: &str,
    op: Op,
    before: Option<&Value>,
    after: Option<&Value>,
    t: &TimeCtx,
) -> String {
    if op == Op::Delete || after.is_none() {
        return match before.and_then(Value::as_object) {
            Some(pk) => generic(pk, t),
            None if op == Op::Delete => "(no primary key in record)".into(),
            None => String::new(),
        };
    }
    let Some(after) = after.and_then(Value::as_object) else {
        return after
            .map(|v| truncate(&v.to_string(), GENERIC_MAX))
            .unwrap_or_default();
    };
    let templated = match table {
        "comments" => comment(after, t),
        "posts" => post(after, t),
        "users" => user(after),
        _ => None,
    };
    templated.unwrap_or_else(|| generic(after, t))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn utc(today: &str) -> TimeCtx {
        TimeCtx {
            offset: FixedOffset::east_opt(0),
            today: today.parse().unwrap(),
        }
    }

    #[test]
    fn at_columns_are_microsecond_times() {
        // 2024-03-15T13:50:00.123456Z, the timestamptz example from the docs.
        let us = 1_710_510_600_123_456;
        assert_eq!(fmt_micros(us, &utc("2024-03-15")), "13:50:00.123");
        assert_eq!(fmt_micros(us, &utc("2024-03-16")), "2024-03-15 13:50:00");
        let plus_two = TimeCtx {
            offset: FixedOffset::east_opt(2 * 3600),
            today: "2024-03-15".parse().unwrap(),
        };
        assert_eq!(fmt_micros(us, &plus_two), "15:50:00.123");
        assert_eq!(fmt_micros(TS_PLUS_INFINITY, &utc("2024-03-15")), "infinity");
        assert_eq!(
            fmt_micros(TS_MINUS_INFINITY, &utc("2024-03-15")),
            "-infinity"
        );

        let t = utc("2024-03-15");
        assert_eq!(fmt_value("created_at", &json!(us), &t), "13:50:00.123");
        // Only *_at columns: other numbers stay numbers.
        assert_eq!(fmt_value("quantity", &json!(us), &t), "1710510600123456");
        assert_eq!(fmt_value("created_at", &json!(null), &t), "null");
    }

    #[test]
    fn json_columns_arrive_as_strings_and_print_compactly() {
        let t = utc("2024-03-15");
        let raw = "{\"category\": \"databases\",\n  \"tags\": [\"dsql\", \"postgres\"]}";
        assert_eq!(
            fmt_value("metadata", &json!(raw), &t),
            r#"{"category":"databases","tags":["dsql","postgres"]}"#
        );
        assert_eq!(fmt_value("j", &json!("[1, 2,  3]"), &t), "[1,2,3]");
        // Long JSON is truncated.
        let big = format!("{{\"k\": \"{}\"}}", "x".repeat(200));
        let out = fmt_value("j", &json!(big), &t);
        assert_eq!(out.chars().count(), JSON_MAX);
        assert!(out.ends_with('…'));
        // Text that merely starts with a brace is still text.
        assert_eq!(fmt_value("body", &json!("{not json"), &t), "\"{not json\"");
    }

    #[test]
    fn other_values() {
        let t = utc("2024-03-15");
        assert_eq!(
            fmt_value("id", &json!("550e8400-e29b-41d4-a716-446655440000"), &t),
            "550e8400…"
        );
        assert_eq!(fmt_value("price", &json!("29.99"), &t), "29.99");
        assert_eq!(fmt_value("price", &json!("NaN"), &t), "NaN");
        assert_eq!(fmt_value("b", &json!(true), &t), "true");
        assert_eq!(fmt_value("v", &json!(3.25), &t), "3.25");
        assert_eq!(
            fmt_value("big", &json!(9_223_372_036_854_775_807i64), &t),
            "9223372036854775807"
        );
        assert_eq!(fmt_value("vec", &json!([]), &t), "[]");
        assert_eq!(fmt_value("s", &json!("two\nlines"), &t), "\"two↵lines\"");
        assert_eq!(fmt_value("s", &json!("2024-01-01"), &t), "\"2024-01-01\"");
        let long = "é".repeat(100);
        let q = fmt_value("s", &json!(long), &t);
        assert_eq!(q.chars().count(), TEXT_MAX + 2);
        assert!(q.ends_with("…\""));
    }

    #[test]
    fn truncation_is_char_based() {
        assert_eq!(truncate("hello", 5), "hello");
        assert_eq!(truncate("hello!", 5), "hell…");
        assert_eq!(truncate("✓✓✓✓✓✓", 3), "✓✓…");
        assert_eq!(truncate("", 3), "");
        assert_eq!(truncate("abc", 0), "");
        assert_eq!(truncate("abc", 1), "…");
    }

    #[test]
    fn uuid_detection() {
        assert!(is_uuid("550e8400-e29b-41d4-a716-446655440000"));
        assert!(!is_uuid("550e8400e29b41d4a716446655440000"));
        assert!(!is_uuid("550e8400-e29b-41d4-a716-44665544000g"));
    }

    #[test]
    fn bytes() {
        assert_eq!(fmt_bytes(812), "812 B");
        assert_eq!(fmt_bytes(12_595), "12.3 KB");
        assert_eq!(fmt_bytes(9 * 1024 * 1024 + 1), "9.0 MB");
    }

    #[test]
    fn blog_summaries() {
        let t = utc("2026-10-14");
        let comment = json!({
            "id": "1b0e7c4e-3f1d-4d0a-9a51-0f6f4c1e2b3a",
            "post_id": "9c1d4e5f-0000-4000-8000-000000000001",
            "user_id": "3f2a1b2c-0000-4000-8000-000000000002",
            "body": "Great post!",
            "created_at": 1_760_450_531_482_000i64
        });
        assert_eq!(
            row_summary("comments", Op::Insert, None, Some(&comment), &t),
            "\"Great post!\" by user 3f2a1b2c… on post 9c1d4e5f…"
        );

        let post = json!({
            "id": "9c1d4e5f-0000-4000-8000-000000000001",
            "author_id": "3f2a1b2c-0000-4000-8000-000000000002",
            "slug": "hello",
            "title": "Building a blog on Aurora DSQL",
            "body": "x".repeat(2048),
            "metadata": "{\"category\": \"databases\", \"tags\": [\"dsql\", \"postgres\", \"occ\", \"extra\"], \"readingMinutes\": 6, \"seo\": {\"description\": \"d\"}}",
            "created_at": 1_760_450_531_482_000i64
        });
        assert_eq!(
            row_summary("posts", Op::Update, None, Some(&post), &t),
            "\"Building a blog on Aurora DSQL\" by 3f2a1b2c… [databases] #dsql #postgres #occ · 6 min · body 2.0 KB"
        );

        let user = json!({"id": "3f2a1b2c-0000-4000-8000-000000000002", "username": "marc", "display_name": "Marc Bowes", "created_at": 0});
        assert_eq!(
            row_summary("users", Op::Insert, None, Some(&user), &t),
            "@marc \"Marc Bowes\""
        );
    }

    #[test]
    fn delete_shows_primary_key() {
        let t = utc("2026-10-14");
        let pk = json!({"id": "1b0e7c4e-3f1d-4d0a-9a51-0f6f4c1e2b3a"});
        assert_eq!(
            row_summary("comments", Op::Delete, Some(&pk), None, &t),
            "id=1b0e7c4e…"
        );
        let composite = json!({"order_id": 1001, "item_id": 42});
        assert_eq!(
            row_summary("order_items", Op::Delete, Some(&composite), None, &t),
            "order_id=1001 item_id=42"
        );
        assert_eq!(
            row_summary("t", Op::Delete, None, None, &t),
            "(no primary key in record)"
        );
    }

    #[test]
    fn unknown_tables_and_unexpected_shapes_fall_back() {
        let t = utc("2026-10-14");
        let row = json!({"order_id": 1001, "item_id": 42, "quantity": 5, "price": "29.99"});
        assert_eq!(
            row_summary("order_items", Op::Insert, None, Some(&row), &t),
            "order_id=1001 item_id=42 quantity=5 price=29.99"
        );
        // A comments table without the expected columns.
        let odd = json!({"id": 7, "text": "hi"});
        assert_eq!(
            row_summary("comments", Op::Insert, None, Some(&odd), &t),
            "id=7 text=\"hi\""
        );
        // Overflowed interval emitted as null in a NOT NULL column.
        let nulls = json!({"id": 1, "elapsed": null});
        assert_eq!(
            row_summary("x", Op::Update, None, Some(&nulls), &t),
            "id=1 elapsed=null"
        );
    }
}
