//! Generated request bodies. Everything here is plain lowercase ASCII, so the
//! JSON can be written directly without an escaping pass.

use std::fmt::Write;

const WORDS: &[&str] = &[
    "aurora",
    "dsql",
    "serverless",
    "postgres",
    "distributed",
    "consistency",
    "latency",
    "throughput",
    "region",
    "replica",
    "commit",
    "snapshot",
    "isolation",
    "optimistic",
    "concurrency",
    "transaction",
    "index",
    "query",
    "lambda",
    "scale",
    "quorum",
    "journal",
    "storage",
    "compute",
    "range",
    "clock",
    "timestamp",
    "conflict",
    "retry",
    "schema",
    "async",
    "cluster",
    "endpoint",
    "token",
    "witness",
    "durable",
    "partition",
    "rust",
    "node",
    "drizzle",
    "adapter",
    "connection",
    "pool",
    "burst",
    "quota",
    "metric",
    "dashboard",
    "benchmark",
    "tuning",
    "cache",
    "batch",
    "stream",
    "change",
    "capture",
    "kinesis",
    "event",
    "record",
    "lag",
    "restore",
    "backup",
    "recovery",
    "upgrade",
    "version",
    "feature",
    "demo",
    "stage",
    "code",
    "prompt",
    "agent",
    "review",
    "deploy",
    "stack",
    "edge",
    "browser",
    "react",
    "page",
    "blog",
    "post",
    "comment",
    "author",
    "reader",
    "category",
    "lesson",
    "pattern",
    "design",
    "system",
    "network",
    "active",
    "writes",
    "reads",
    "strong",
    "fast",
    "simple",
    "elastic",
    "global",
    "local",
    "cold",
    "warm",
    "start",
    "really",
    "nice",
    "great",
    "clear",
    "useful",
];

const OPENERS: &[&str] = &[
    "Great post.",
    "Thanks for writing this up.",
    "This matches what we saw in production.",
    "Interesting take.",
    "Bookmarked.",
    "Nice explanation.",
    "Question:",
    "Agreed.",
];

fn word() -> &'static str {
    WORDS[fastrand::usize(..WORDS.len())]
}

fn push_words(out: &mut String, n: usize) {
    for i in 0..n {
        if i > 0 {
            out.push(' ');
        }
        out.push_str(word());
    }
}

fn push_sentence(out: &mut String, min: usize, max: usize) {
    let start = out.len();
    push_words(out, fastrand::usize(min..=max));
    // Capitalise the first letter (ASCII only).
    if let Some(c) = out[start..].chars().next() {
        out.replace_range(start..start + 1, &c.to_ascii_uppercase().to_string());
    }
    out.push('.');
}

/// `{"userId":"…","body":"…"}` for `POST /posts/:id/comments`.
pub fn comment_json(out: &mut String, user_id: &str) {
    out.clear();
    out.push_str("{\"userId\":\"");
    out.push_str(user_id);
    out.push_str("\",\"body\":\"");
    out.push_str(OPENERS[fastrand::usize(..OPENERS.len())]);
    out.push(' ');
    push_sentence(out, 5, 16);
    out.push_str("\"}");
}

/// `{"authorId","title","body","category","tags"}` for `POST /posts`.
pub fn post_json(author_id: &str, category: &str) -> String {
    let mut out = String::with_capacity(1024);
    out.push_str("{\"authorId\":\"");
    out.push_str(author_id);
    out.push_str("\",\"title\":\"");
    // Titles are random words plus a short token: easy to spot as load-test
    // posts in the UI, and unlikely to collide if the API derives slugs from them.
    let start = out.len();
    push_words(&mut out, fastrand::usize(3..=6));
    if let Some(c) = out[start..].chars().next() {
        out.replace_range(start..start + 1, &c.to_ascii_uppercase().to_string());
    }
    let _ = write!(out, " (load test {:06x})", fastrand::u32(..0x100_0000));
    out.push_str("\",\"body\":\"");
    let paragraphs = fastrand::usize(2..=4);
    for p in 0..paragraphs {
        if p > 0 {
            out.push_str("\\n\\n");
        }
        for s in 0..fastrand::usize(3..=6) {
            if s > 0 {
                out.push(' ');
            }
            push_sentence(&mut out, 6, 18);
        }
    }
    out.push_str("\",\"category\":\"");
    out.push_str(category);
    out.push_str("\",\"tags\":[");
    for t in 0..fastrand::usize(1..=3) {
        if t > 0 {
            out.push(',');
        }
        out.push('"');
        out.push_str(word());
        out.push('"');
    }
    out.push_str("]}");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comment_is_valid_json() {
        let mut buf = String::new();
        for _ in 0..1000 {
            comment_json(&mut buf, "8a1f3c2e-0000-4000-8000-000000000001");
            let v: serde_json::Value = serde_json::from_str(&buf).expect(&buf);
            assert_eq!(v["userId"], "8a1f3c2e-0000-4000-8000-000000000001");
            assert!(v["body"].as_str().unwrap().len() > 10);
        }
    }

    #[test]
    fn post_is_valid_json() {
        for _ in 0..500 {
            let s = post_json("u-1", "databases");
            let v: serde_json::Value = serde_json::from_str(&s).expect(&s);
            assert_eq!(v["authorId"], "u-1");
            assert_eq!(v["category"], "databases");
            assert!(v["title"].as_str().unwrap().contains("(load test "));
            assert!(v["body"].as_str().unwrap().contains('.'));
            assert!(!v["tags"].as_array().unwrap().is_empty());
        }
    }
}
