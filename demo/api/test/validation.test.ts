import assert from "node:assert/strict";
import { describe, it } from "node:test";
import {
  HttpError,
  buildPostMeta,
  excerpt,
  isUuid,
  normalizeTags,
  parseBefore,
  parseCategory,
  parseLimit,
  slugify,
  truncate,
  validateNewComment,
  validateNewPost,
} from "../src/validation.js";
import { pgTimestampToIso } from "../src/time.js";

const UUID = "0b9d6f7e-3c55-4a43-9f53-2f3c7f0a1b2c";

function status(fn: () => unknown): number | undefined {
  try {
    fn();
  } catch (err) {
    if (err instanceof HttpError) return err.status;
    throw err;
  }
  return undefined;
}

describe("query params", () => {
  it("parseLimit", () => {
    assert.equal(parseLimit(undefined, 20, 100), 20);
    assert.equal(parseLimit("", 20, 100), 20);
    assert.equal(parseLimit("5", 20, 100), 5);
    assert.equal(parseLimit("100", 20, 100), 100);
    for (const bad of ["0", "101", "-1", "2.5", "abc", "1e2"]) assert.equal(status(() => parseLimit(bad, 20, 100)), 400, bad);
  });

  it("parseBefore keeps the cursor exact (microseconds included)", () => {
    assert.equal(parseBefore(undefined), undefined);
    assert.equal(parseBefore("2026-12-01T17:00:00.000Z"), "2026-12-01T17:00:00.000Z");
    assert.equal(parseBefore("2026-12-01T17:00:00.123456Z"), "2026-12-01T17:00:00.123456Z");
    assert.equal(parseBefore("2026-12-01T12:00:00-05:00"), "2026-12-01T12:00:00-05:00");
    for (const bad of ["yesterday", "2026", "2026-12-01", "2026-13-45T99:99Z", "2026-12-01T17:00:00", "1; drop table posts"]) {
      assert.equal(status(() => parseBefore(bad)), 400, bad);
    }
  });

  it("parseCategory", () => {
    assert.equal(parseCategory(undefined), undefined);
    assert.equal(parseCategory("databases"), "databases");
    assert.equal(status(() => parseCategory("Databases")), 400);
    assert.equal(status(() => parseCategory("cooking")), 400);
  });
});

describe("bodies", () => {
  it("validateNewPost accepts and normalizes", () => {
    const p = validateNewPost({
      authorId: UUID,
      title: "  Hello DSQL  ",
      body: "Body text",
      category: "databases",
      tags: ["Multi Region", "multi-region", "  OCC "],
    });
    assert.deepEqual(p, {
      authorId: UUID,
      title: "Hello DSQL",
      body: "Body text",
      category: "databases",
      tags: ["multi-region", "occ"],
    });
  });

  it("validateNewPost reports every problem", () => {
    try {
      validateNewPost({ authorId: "nope", title: "", category: "x", tags: "a" });
      assert.fail("expected HttpError");
    } catch (err) {
      assert.ok(err instanceof HttpError);
      assert.equal(err.status, 400);
      assert.deepEqual(err.details, [
        "authorId must be a uuid",
        "title is required",
        "body is required",
        "category must be one of: databases, distributed-systems, serverless, performance, security, devops, frontend, career",
        "tags must be an array of strings",
      ]);
    }
    assert.equal(status(() => validateNewPost(null)), 400);
    assert.equal(status(() => validateNewPost([1])), 400);
  });

  it("validateNewComment", () => {
    assert.deepEqual(validateNewComment({ userId: UUID, body: " nice post " }), { userId: UUID, body: "nice post" });
    assert.equal(status(() => validateNewComment({ userId: UUID, body: "   " })), 400);
    assert.equal(status(() => validateNewComment({ userId: UUID, body: "x".repeat(2001) })), 400);
    assert.equal(status(() => validateNewComment({ userId: 42, body: "hi" })), 400);
  });

  it("normalizeTags", () => {
    assert.deepEqual(normalizeTags(undefined), []);
    assert.equal(normalizeTags([1, 2]), undefined);
    assert.deepEqual(normalizeTags(["C++ / Rust", "", "!!"]), ["c-rust"]);
  });

  it("isUuid", () => {
    assert.ok(isUuid(UUID));
    assert.ok(isUuid(UUID.toUpperCase()));
    assert.ok(!isUuid(`${UUID}x`));
    assert.ok(!isUuid("1; drop table posts"));
  });
});

describe("text helpers", () => {
  it("slugify", () => {
    assert.equal(slugify("Café: Ünïcode & OCC, explained!", "abc123"), "cafe-unicode-occ-explained-abc123");
    assert.equal(slugify("!!!", "x"), "post-x");
    assert.ok(slugify("a ".repeat(200), "s").length <= 72 + 2);
  });

  it("truncate / excerpt", () => {
    assert.equal(truncate("short", 10), "short");
    assert.equal(truncate("## Heading\n\nsome   words here", 100), "Heading some words here");
    const long = "The quick brown fox jumps over the lazy dog. ".repeat(20);
    const e = excerpt(long);
    assert.ok(e.length <= 221, String(e.length));
    assert.ok(e.endsWith("…"));
    assert.ok(!e.includes("  "));
  });

  it("buildPostMeta", () => {
    const m = buildPostMeta("word ".repeat(1100), "serverless", ["lambda"]);
    assert.equal(m.category, "serverless");
    assert.deepEqual(m.tags, ["lambda"]);
    assert.equal(m.readingMinutes, 5);
    assert.ok(m.seo.description.length <= 156);
  });
});

describe("timestamps", () => {
  it("pgTimestampToIso keeps microseconds and normalizes to UTC", () => {
    assert.equal(pgTimestampToIso("2026-10-14 12:00:00.123456+00"), "2026-10-14T12:00:00.123456Z");
    assert.equal(pgTimestampToIso("2026-10-14 05:00:00.123456-07"), "2026-10-14T12:00:00.123456Z");
    assert.equal(pgTimestampToIso("2026-10-14 17:30:00.5+05:30"), "2026-10-14T12:00:00.5Z");
    assert.equal(pgTimestampToIso("2026-12-31 23:59:59+00"), "2026-12-31T23:59:59Z");
    assert.equal(pgTimestampToIso("2027-01-01 00:59:59.000001+01"), "2026-12-31T23:59:59.000001Z");
    assert.equal(pgTimestampToIso("2026-10-14T12:00:00.123Z"), "2026-10-14T12:00:00.123Z");
  });
});
