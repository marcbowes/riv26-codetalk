import assert from "node:assert/strict";
import { describe, it } from "node:test";
import { CATEGORIES } from "@dat401/api/categories";
import { BATCH } from "../steps.js";
import { chunk, generateComments, generatePosts, generateUsers, uuid, rng } from "../seed.js";

const NOW = new Date("2026-11-30T17:00:00Z");
const UUID_RE = /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/;

describe("seed generation", () => {
  const users = generateUsers(500, 401, NOW);
  const ids = users.map((u) => u.id).sort();
  const posts = generatePosts(ids, 5000, 402, NOW);
  const comments = generateComments(posts, ids, 30, 403, NOW);

  it("is deterministic", () => {
    assert.deepEqual(generateUsers(500, 401, NOW), users);
    assert.deepEqual(generatePosts(ids, 50, 402, NOW), posts.slice(0, 50));
  });

  it("produces valid v4 uuids", () => {
    const r = rng(1);
    for (let i = 0; i < 1000; i++) assert.match(uuid(r), UUID_RE);
  });

  it("unique usernames, slugs and ids", () => {
    assert.equal(new Set(users.map((u) => u.username)).size, users.length);
    assert.equal(new Set(posts.map((p) => p.slug)).size, posts.length);
    const all = [...users.map((u) => u.id), ...posts.map((p) => p.id), ...comments.map((c) => c.commentId)];
    assert.equal(new Set(all).size, all.length);
  });

  it("plausible posts", () => {
    const cats = new Set(posts.map((p) => p.metadata.category));
    assert.deepEqual([...cats].sort(), [...CATEGORIES].sort());
    for (const p of posts) {
      assert.ok(p.title.length > 5 && p.title.length <= 200, p.title);
      assert.ok(p.body.length > 1_000 && p.body.length < 20_000, String(p.body.length));
      assert.ok(p.metadata.tags.length >= 2 && p.metadata.tags.length <= 4);
      assert.ok(p.metadata.readingMinutes >= 1);
      assert.ok(p.metadata.seo.description.length > 0 && p.metadata.seo.description.length <= 221);
      assert.ok(Date.parse(p.createdAt) < NOW.getTime());
      assert.ok(ids.includes(p.authorId));
    }
  });

  it("comments: 0-30 per post, after the post, before now", () => {
    const byPost = new Map<string, number>();
    const postTime = new Map(posts.map((p) => [p.id, Date.parse(p.createdAt)]));
    for (const c of comments) {
      byPost.set(c.postId, (byPost.get(c.postId) ?? 0) + 1);
      assert.ok(Date.parse(c.createdAt) > (postTime.get(c.postId) as number));
      assert.ok(Date.parse(c.createdAt) <= NOW.getTime() + 60_000);
    }
    assert.ok(Math.max(...byPost.values()) <= 30);
    assert.ok(byPost.size < posts.length, "some posts have no comments");
    const avg = comments.length / posts.length;
    assert.ok(avg > 8 && avg < 18, `avg ${avg}`);
  });

  it("every row has its own created_at; posts span about two years, after their authors joined", () => {
    for (const rows of [users, posts, comments]) {
      assert.equal(new Set(rows.map((x) => x.createdAt)).size, rows.length);
    }
    const times = posts.map((p) => Date.parse(p.createdAt));
    const spanDays = (Math.max(...times) - Math.min(...times)) / 86_400_000;
    assert.ok(spanDays > 600 && spanDays <= 731, `span ${spanDays.toFixed(0)} days`);
    const newestUser = Math.max(...users.map((u) => Date.parse(u.createdAt)));
    assert.ok(newestUser < Math.min(...times));
  });

  it("batches respect DSQL's 3,000 row / 10 MiB transaction limits", () => {
    for (const [rows, size] of [
      [users, BATCH.users],
      [posts, BATCH.posts],
      [comments, BATCH.comments],
    ] as const) {
      for (const batch of chunk(rows as readonly object[], size)) {
        assert.ok(batch.length <= 1000);
        const bytes = Buffer.byteLength(JSON.stringify(batch));
        assert.ok(bytes < 4 * 1024 * 1024, `batch of ${batch.length} is ${bytes} bytes`);
      }
    }
  });
});
