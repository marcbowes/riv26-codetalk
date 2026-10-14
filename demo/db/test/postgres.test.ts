// Integration test against a plain local PostgreSQL (not DSQL): exercises the
// setup steps that are standard SQL, the seed, and every API route end to end.
// DSQL-only pieces (CREATE INDEX ASYNC, sys.wait_for_job, AWS IAM GRANT) can't
// run here.
//
//   PG_TEST_URL=postgres://postgres@localhost:55432/postgres npm test -w @dat401/db
import assert from "node:assert/strict";
import { after, before, describe, it } from "node:test";
import { drizzle } from "@aws/aurora-dsql-drizzle";
import { createApp } from "@dat401/api/app";
import { listPostsQuery } from "@dat401/api/queries";
import * as schema from "@dat401/api/schema";
import pg from "pg";
import { INDEXES } from "../ddl.js";
import { ensureRole, ensureTables, querier, seedAll, type Q } from "../steps.js";

const url = process.env.PG_TEST_URL;

describe("setup + API against local Postgres", { skip: url ? false : "set PG_TEST_URL to run" }, () => {
  let pool: pg.Pool;
  let q: Q;
  const counts = async () => {
    const r = await pool.query(
      `SELECT (SELECT count(*)::int FROM users) AS users, (SELECT count(*)::int FROM posts) AS posts,
              (SELECT count(*)::int FROM comments) AS comments`,
    );
    return r.rows[0] as { users: number; posts: number; comments: number };
  };

  before(async () => {
    pool = new pg.Pool({ connectionString: url, max: 6 });
    q = querier(pool);
    await pool.query("DROP TABLE IF EXISTS comments, posts, users");
  });
  after(async () => {
    await pool?.end();
  });

  it("ensureTables creates, then is a no-op", async () => {
    await ensureTables(q);
    await ensureTables(q);
    const r = await pool.query(
      `SELECT conname FROM pg_constraint WHERE contype = 'f' AND conrelid IN ('posts'::regclass, 'comments'::regclass) ORDER BY 1`,
    );
    assert.deepEqual(
      r.rows.map((x) => x.conname),
      ["comments_post_id_fkey", "comments_user_id_fkey", "posts_author_id_fkey"],
    );
  });

  it("index definitions are valid SQL (plain CREATE INDEX here)", async () => {
    for (const ix of INDEXES) await pool.query(`CREATE INDEX IF NOT EXISTS ${ix.name} ON ${ix.table} ${ix.on}`);
  });

  it("ensureRole is idempotent", async () => {
    await ensureRole(q, "blog_app");
    await ensureRole(q, "blog_app");
    const r = await pool.query(`SELECT has_table_privilege('blog_app', 'comments', 'INSERT') AS ok`);
    assert.equal(r.rows[0].ok, true);
  });

  const seedOpts = { resume: false, users: 120, posts: 600, maxComments: 30, concurrency: 4, seedValue: 401 };

  it("seedAll seeds empty tables, skips non-empty ones, and --resume adds nothing new", async () => {
    await seedAll(pool, q, seedOpts);
    const first = await counts();
    assert.equal(first.users, 120);
    assert.equal(first.posts, 600);
    assert.ok(first.comments > 3000, String(first.comments));
    await seedAll(pool, q, seedOpts);
    assert.deepEqual(await counts(), first);
    await seedAll(pool, q, { ...seedOpts, resume: true });
    assert.deepEqual(await counts(), first);
  });

  describe("API", () => {
    const db = () => drizzle({ client: pool, schema }) as any;
    const app = createApp({ getDb: db, region: "pg-test", cors: true });
    const get = async (path: string) => {
      const res = await app.request(path);
      return { status: res.status, body: (await res.json()) as any };
    };
    const post = async (path: string, body: unknown) => {
      const res = await app.request(path, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify(body),
      });
      return { status: res.status, body: (await res.json()) as any };
    };

    it("GET /health", async () => {
      const { status, body } = await get("/health");
      assert.equal(status, 200);
      assert.equal(body.ok, true);
      assert.ok(body.timing.dbMs > 0);
    });

    it("GET /users", async () => {
      const { body } = await get("/users?limit=5");
      assert.equal(body.users.length, 5);
      assert.deepEqual(Object.keys(body.users[0]).sort(), ["displayName", "id", "username"]);
    });

    it("GET /posts: newest first, excerpts, authors, cursor, category", async () => {
      const { body } = await get("/posts?limit=10");
      assert.equal(body.posts.length, 10);
      const p = body.posts[0];
      assert.deepEqual(Object.keys(p).sort(), [
        "author", "category", "createdAt", "excerpt", "id", "readingMinutes", "slug", "tags", "title",
      ]);
      assert.ok(p.excerpt.length <= 221);
      const times = body.posts.map((x: any) => Date.parse(x.createdAt));
      assert.deepEqual(times, [...times].sort((a, b) => b - a));
      const older = await get(`/posts?limit=10&before=${encodeURIComponent(body.posts[9].createdAt)}`);
      assert.ok(older.body.posts.every((x: any) => Date.parse(x.createdAt) < times[9]));
      const cat = await get("/posts?category=databases&limit=50");
      assert.ok(cat.body.posts.length > 0);
      assert.ok(cat.body.posts.every((x: any) => x.category === "databases"));
    });

    it("GET /posts/:id with comments oldest first; 404 when missing", async () => {
      const { body: list } = await get("/posts?limit=50");
      const id = list.posts[0].id;
      const { status, body } = await get(`/posts/${id}`);
      assert.equal(status, 200);
      assert.equal(body.post.id, id);
      assert.ok(body.post.body.length > 0);
      assert.equal(body.post.metadata.category, body.post.category);
      const times = body.comments.map((c: any) => Date.parse(c.createdAt));
      assert.deepEqual(times, [...times].sort((a, b) => a - b));
      const missing = await get("/posts/00000000-0000-4000-8000-000000000000");
      assert.equal(missing.status, 404);
    });

    it("POST /posts and POST /posts/:id/comments", async () => {
      const { body: u } = await get("/users?limit=2");
      const [alice, bob] = u.users;
      const created = await post("/posts", {
        authorId: alice.id,
        title: "Hello from the integration test",
        body: "First paragraph.\n\nSecond paragraph with more words in it.",
        category: "databases",
        tags: ["Testing", "dsql"],
      });
      assert.equal(created.status, 201, JSON.stringify(created.body));
      assert.equal(created.body.post.author.username, alice.username);
      assert.deepEqual(created.body.post.tags, ["testing", "dsql"]);
      assert.match(created.body.post.slug, /^hello-from-the-integration-test-[0-9a-f]{8}$/);

      const id = created.body.post.id;
      const c = await post(`/posts/${id}/comments`, { userId: bob.id, body: "Nice!" });
      assert.equal(c.status, 201, JSON.stringify(c.body));
      assert.equal(c.body.comment.user.displayName, bob.displayName);
      const { body: detail } = await get(`/posts/${id}`);
      assert.equal(detail.comments.at(-1).id, c.body.comment.id);
      const listed = await get("/posts?limit=1");
      assert.equal(listed.body.posts[0].id, id);

      const noUser = await post(`/posts/${id}/comments`, { userId: "00000000-0000-4000-8000-000000000000", body: "x" });
      assert.equal(noUser.status, 400);
      const noPost = await post(`/posts/00000000-0000-4000-8000-000000000000/comments`, { userId: bob.id, body: "x" });
      assert.equal(noPost.status, 404);
      const noAuthor = await post("/posts", {
        authorId: "00000000-0000-4000-8000-000000000000",
        title: "t",
        body: "b",
        category: "career",
      });
      assert.equal(noAuthor.status, 400);
    });

    it("?before= pages exactly, even between rows 1 microsecond apart", async () => {
      const { body: u } = await get("/users?limit=1");
      const meta = JSON.stringify({ category: "career", tags: [], readingMinutes: 1, seo: { description: "" } });
      await pool.query(
        `INSERT INTO posts (author_id, slug, title, body, metadata, created_at) VALUES
           ($1, 'micro-1', 'micro 1', 'b', $2, now() + interval '1 day' + interval '3 microseconds'),
           ($1, 'micro-2', 'micro 2', 'b', $2, now() + interval '1 day' + interval '2 microseconds'),
           ($1, 'micro-3', 'micro 3', 'b', $2, now() + interval '1 day' + interval '1 microsecond')`,
        [u.users[0].id, meta],
      );
      const seen: string[] = [];
      let before: string | undefined;
      for (let i = 0; i < 3; i++) {
        const { body } = await get(`/posts?limit=1${before ? `&before=${encodeURIComponent(before)}` : ""}`);
        seen.push(body.posts[0].title);
        before = body.posts[0].createdAt;
        assert.match(before as string, /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{6}Z$/);
      }
      assert.deepEqual(seen, ["micro 1", "micro 2", "micro 3"]);
    });

    it("the generated category query can use posts_category_idx", async () => {
      const { sql, params } = listPostsQuery(db(), { limit: 20, category: "databases" }).toSQL();
      const client = await pool.connect();
      try {
        await client.query("ANALYZE posts");
        await client.query("SET enable_seqscan = off");
        const plan = await client.query(`EXPLAIN ${sql}`, params);
        const text = plan.rows.map((r) => r["QUERY PLAN"]).join("\n");
        assert.match(text, /posts_category_idx/, text);
        await client.query("RESET enable_seqscan");
      } finally {
        client.release();
      }
    });
  });
});
