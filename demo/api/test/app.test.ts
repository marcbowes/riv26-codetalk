// Routing, validation, CORS and the response envelope, without a database.
import assert from "node:assert/strict";
import { describe, it } from "node:test";
import { drizzle } from "@aws/aurora-dsql-drizzle";
import { createApp } from "../src/app.js";
import type { Db } from "../src/db.js";
import { listPostsQuery } from "../src/queries.js";
import * as schema from "../src/schema.js";
import { HttpError } from "../src/validation.js";

const UUID = "0b9d6f7e-3c55-4a43-9f53-2f3c7f0a1b2c";

let dbCalls = 0;
const noDb = () => {
  dbCalls++;
  throw new HttpError(503, "database not configured: set CLUSTER_ENDPOINT");
};

const app = createApp({ getDb: noDb, region: "test-region", cors: true });
const lambdaApp = createApp({ getDb: noDb, region: "us-east-1", cors: false });

async function call(path: string, init?: RequestInit, a = app) {
  const res = await a.request(path, init);
  const body = res.headers.get("content-type")?.includes("json") ? await res.json() : undefined;
  return { res, body: body as Record<string, any> };
}

const post = (body: unknown) => ({
  method: "POST",
  headers: { "content-type": "application/json", origin: "https://example.cloudfront.net" },
  body: typeof body === "string" ? body : JSON.stringify(body),
});

describe("envelope", () => {
  it("GET /categories has region, timing and x-region", async () => {
    const { res, body } = await call("/categories");
    assert.equal(res.status, 200);
    assert.equal(res.headers.get("x-region"), "test-region");
    assert.equal(body.region, "test-region");
    assert.deepEqual(body.timing, { dbMs: 0 });
    assert.equal(body.categories.length, 8);
  });

  it("unknown routes are JSON 404s with the envelope", async () => {
    const { res, body } = await call("/nope");
    assert.equal(res.status, 404);
    assert.equal(body.region, "test-region");
    assert.match(body.error, /no route/);
    assert.equal(res.headers.get("x-region"), "test-region");
  });
});

describe("validation happens before the database is touched", () => {
  const cases: [string, RequestInit | undefined, number][] = [
    ["/posts?limit=abc", undefined, 400],
    ["/posts?limit=1000", undefined, 400],
    ["/posts?category=cooking", undefined, 400],
    ["/posts?before=yesterday", undefined, 400],
    ["/users?limit=0", undefined, 400],
    ["/posts/not-a-uuid", undefined, 404],
    [`/posts/not-a-uuid/comments`, post({ userId: UUID, body: "hi" }), 404],
    [`/posts/${UUID}/comments`, post("{not json"), 400],
    [`/posts/${UUID}/comments`, post({ userId: "x", body: "" }), 400],
    ["/posts", post({ title: "t" }), 400],
  ];
  for (const [path, init, expected] of cases) {
    it(`${init?.method ?? "GET"} ${path} -> ${expected}`, async () => {
      const before = dbCalls;
      const { res, body } = await call(path, init);
      assert.equal(res.status, expected, JSON.stringify(body));
      assert.equal(typeof body.error, "string");
      assert.equal(body.region, "test-region");
      assert.equal(dbCalls, before, "database should not be touched");
    });
  }

  it("400s list every problem", async () => {
    const { body } = await call(`/posts/${UUID}/comments`, post({ userId: "x", body: "" }));
    assert.deepEqual(body.details, ["userId must be a uuid", "body is required"]);
  });

  it("oversized bodies are rejected", async () => {
    const { res } = await call(`/posts/${UUID}/comments`, post({ userId: UUID, body: "x".repeat(70_000) }));
    assert.equal(res.status, 413);
  });
});

describe("database errors", () => {
  it("/health reports a clear error when the DB is unavailable", async () => {
    const { res, body } = await call("/health");
    assert.equal(res.status, 503);
    assert.equal(body.ok, false);
    assert.match(body.error, /CLUSTER_ENDPOINT/);
  });

  it("valid requests reach the DB layer and surface 503", async () => {
    const { res, body } = await call(`/posts/${UUID}/comments`, post({ userId: UUID, body: "hello" }));
    assert.equal(res.status, 503);
    assert.match(body.error, /CLUSTER_ENDPOINT/);
  });
});

describe("DSQL connection limits -> 429", () => {
  const limited = (code: string | undefined, message: string) =>
    createApp({
      getDb: () => {
        // Shaped like what drizzle surfaces: DrizzleQueryError with the pg error as cause.
        const pgErr = Object.assign(new Error(message), code ? { code } : {});
        throw Object.assign(new Error("Failed query: select 1"), { cause: pgErr });
      },
      region: "us-east-1",
      cors: false,
    });

  for (const [code, message] of [
    ["53400", "Unable to accept connection, rate exceeded."],
    ["53300", "Unable to accept connection, too many open connections."],
    [undefined, "Unable to accept connection, rate exceeded."],
  ] as const) {
    it(`${code ?? "message only"} on a normal route`, async () => {
      const res = await limited(code, message).request("/posts");
      assert.equal(res.status, 429);
      assert.equal(res.headers.get("x-amzn-ErrorType"), "TooManyRequestsException");
      assert.equal(res.headers.get("retry-after"), "1");
      const body = (await res.json()) as any;
      assert.match(body.error, /connection limit/);
      assert.equal(body.region, "us-east-1");
    });
  }

  it("on /health too", async () => {
    const res = await limited("53400", "Unable to accept connection, rate exceeded.").request("/health");
    assert.equal(res.status, 429);
    assert.equal(res.headers.get("x-amzn-ErrorType"), "TooManyRequestsException");
  });

  it("other DB errors stay 5xx", async () => {
    const res = await limited("57P01", "terminating connection due to administrator command").request("/posts");
    assert.equal(res.status, 503);
    assert.equal(res.headers.get("x-amzn-ErrorType"), null);
  });
});

describe("CORS", () => {
  it("local app answers preflight", async () => {
    const res = await app.request("/posts/x/comments", {
      method: "OPTIONS",
      headers: {
        origin: "http://localhost:5173",
        "access-control-request-method": "POST",
        "access-control-request-headers": "content-type",
      },
    });
    assert.equal(res.status, 204);
    assert.equal(res.headers.get("access-control-allow-origin"), "*");
    assert.match(res.headers.get("access-control-allow-methods") ?? "", /POST/);
    assert.match(res.headers.get("access-control-allow-headers") ?? "", /content-type/i);
  });

  it("local app adds ACAO and exposes x-region on normal responses", async () => {
    const res = await app.request("/categories", { headers: { origin: "http://localhost:5173" } });
    assert.equal(res.headers.get("access-control-allow-origin"), "*");
    assert.match(res.headers.get("access-control-expose-headers") ?? "", /x-region/);
  });

  it("Lambda app leaves CORS headers to the function URL (no duplicates)", async () => {
    const res = await lambdaApp.request("/categories", { headers: { origin: "http://localhost:5173" } });
    assert.equal(res.status, 200);
    assert.equal(res.headers.get("access-control-allow-origin"), null);
    const pre = await lambdaApp.request("/posts", { method: "OPTIONS" });
    assert.equal(pre.status, 204);
  });
});

describe("query building", () => {
  const db = drizzle.mock({ schema }) as unknown as Db;

  it("category filter uses the indexed expression metadata->>'category'", () => {
    const { sql, params } = listPostsQuery(db, {
      limit: 20,
      category: "databases",
      before: "2026-12-01T17:00:00.000Z",
    }).toSQL();
    assert.match(sql, /\("posts"\."metadata" ->> 'category'\) = \$1/);
    assert.match(sql, /"posts"\."created_at" < \$2/);
    assert.match(sql, /order by "posts"\."created_at" desc limit \$3/);
    // The list never reads body, so the covering list indexes cover it.
    assert.doesNotMatch(sql, /"posts"\."body"/);
    assert.deepEqual(params, ["databases", "2026-12-01T17:00:00.000Z", 20]);
  });

  it("no filters, no where clause", () => {
    const { sql } = listPostsQuery(db, { limit: 5 }).toSQL();
    assert.doesNotMatch(sql, / where /);
  });
});
