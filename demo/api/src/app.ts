// The blog API. One Hono app, two entrypoints: src/local.ts (node server) and
// src/lambda.ts (Lambda function URL).
import { type Context, Hono } from "hono";
import { bodyLimit } from "hono/body-limit";
import { cors } from "hono/cors";
import { HTTPException } from "hono/http-exception";
import type { ContentfulStatusCode } from "hono/utils/http-status";
import { AwsDsqlRetryExhaustedError } from "@aws/aurora-dsql-drizzle";
import { CATEGORIES } from "./categories.js";
import { type Db, isConnectionLimited, isUnavailable, retryBrokenConnectionOnce, rootMessage } from "./db.js";
import * as q from "./queries.js";
import {
  HttpError,
  isUuid,
  parseBefore,
  parseCategory,
  parseLimit,
  validateNewComment,
  validateNewPost,
} from "./validation.js";

export interface AppOptions {
  /** Returns the (lazily created) database. May throw HttpError(503) if unconfigured. */
  getDb: () => Db;
  /** Reported in every body and the x-region header. */
  region?: string;
  /**
   * Emit CORS headers from the app. On for local dev. Off on Lambda: the
   * function URL's CORS config adds them, and doing both duplicates
   * Access-Control-Allow-Origin on GET/POST (which browsers reject).
   */
  cors?: boolean;
  /** One JSON log line per request. */
  logRequests?: boolean;
}

type Env = { Variables: { dbMs: number } };

const round1 = (n: number) => Math.round(n * 10) / 10;

export function createApp(opts: AppOptions) {
  const region = opts.region ?? process.env.AWS_REGION ?? "local";
  const app = new Hono<Env>();

  /** JSON response with the envelope every body carries: region + timing. */
  const reply = (c: Context<Env>, body: Record<string, unknown>, status: ContentfulStatusCode = 200) => {
    const dbMs = round1(c.get("dbMs") ?? 0);
    c.header("server-timing", `db;dur=${dbMs}`);
    return c.json({ ...body, region, timing: { dbMs } }, status);
  };

  /** Run DB work, adding its wall time to this request's dbMs. */
  const run = async <T>(c: Context<Env>, fn: (db: Db) => Promise<T>): Promise<T> => {
    const db = opts.getDb();
    const start = performance.now();
    try {
      return await retryBrokenConnectionOnce(() => fn(db));
    } finally {
      c.set("dbMs", c.get("dbMs") + (performance.now() - start));
    }
  };

  const readJson = async (c: Context<Env>): Promise<unknown> => {
    try {
      return await c.req.json();
    } catch {
      throw new HttpError(400, "request body must be valid JSON");
    }
  };

  app.use("*", async (c, next) => {
    const start = performance.now();
    c.set("dbMs", 0);
    await next();
    c.res.headers.set("x-region", region);
    if (opts.logRequests) {
      console.log(
        JSON.stringify({
          method: c.req.method,
          path: c.req.path,
          status: c.res.status,
          ms: round1(performance.now() - start),
          dbMs: round1(c.get("dbMs")),
        }),
      );
    }
  });

  if (opts.cors) {
    app.use(
      "*",
      cors({
        origin: "*",
        allowMethods: ["GET", "POST", "OPTIONS"],
        allowHeaders: ["content-type"],
        exposeHeaders: ["x-region", "server-timing", "x-amzn-ErrorType", "retry-after"],
        maxAge: 86_400,
      }),
    );
  } else {
    // If a preflight ever reaches the function, answer it; the function URL
    // supplies the CORS headers.
    app.options("*", (c) => c.body(null, 204));
  }

  app.use("/posts", bodyLimit({ maxSize: 256 * 1024, onError: () => Promise.reject(new HttpError(413, "body too large")) }));
  app.use("/posts/*", bodyLimit({ maxSize: 64 * 1024, onError: () => Promise.reject(new HttpError(413, "body too large")) }));

  /** 429 + the header Lambda itself uses for throttles, so clients back off. */
  const tooManyRequests = (c: Context<Env>, err: unknown, extra: Record<string, unknown> = {}) => {
    c.header("x-amzn-ErrorType", "TooManyRequestsException");
    c.header("retry-after", "1");
    return reply(c, { ...extra, error: `database connection limit reached, retry with backoff: ${rootMessage(err)}` }, 429);
  };

  app.get("/health", async (c) => {
    try {
      await run(c, q.ping);
      return reply(c, { ok: true });
    } catch (err) {
      if (isConnectionLimited(err)) return tooManyRequests(c, err, { ok: false });
      const message = err instanceof HttpError ? err.message : `database unavailable: ${rootMessage(err)}`;
      return reply(c, { ok: false, error: message }, 503);
    }
  });

  app.get("/categories", (c) => reply(c, { categories: CATEGORIES }));

  app.get("/users", async (c) => {
    const limit = parseLimit(c.req.query("limit"), 50, 500);
    const users = await run(c, (db) => q.listUsers(db, limit));
    return reply(c, { users });
  });

  app.get("/posts", async (c) => {
    const limit = parseLimit(c.req.query("limit"), 20, 100);
    const category = parseCategory(c.req.query("category"));
    const before = parseBefore(c.req.query("before"));
    const posts = await run(c, (db) =>
      q.listPosts(db, { limit, ...(category ? { category } : {}), ...(before ? { before } : {}) }),
    );
    return reply(c, { posts });
  });

  app.get("/posts/:id", async (c) => {
    const id = c.req.param("id");
    if (!isUuid(id)) throw new HttpError(404, "post not found");
    const found = await run(c, (db) => q.getPost(db, id));
    if (!found) throw new HttpError(404, "post not found");
    return reply(c, found);
  });

  app.post("/posts", async (c) => {
    const input = validateNewPost(await readJson(c));
    const post = await run(c, (db) => q.createPost(db, input));
    return reply(c, { post }, 201);
  });

  app.post("/posts/:id/comments", async (c) => {
    const postId = c.req.param("id");
    if (!isUuid(postId)) throw new HttpError(404, "post not found");
    const input = validateNewComment(await readJson(c));
    const comment = await run(c, (db) => q.addComment(db, postId, input));
    return reply(c, { comment }, 201);
  });

  app.notFound((c) => reply(c, { error: `no route for ${c.req.method} ${c.req.path}` }, 404));

  app.onError((err, c) => {
    if (err instanceof HttpError) {
      return reply(c, { error: err.message, ...(err.details ? { details: err.details } : {}) }, err.status);
    }
    if (err instanceof HTTPException) {
      return reply(c, { error: err.message || "bad request" }, err.status as ContentfulStatusCode);
    }
    if (isConnectionLimited(err)) {
      console.warn(JSON.stringify({ level: "warn", msg: "connection limited", error: rootMessage(err) }));
      return tooManyRequests(c, err);
    }
    if (err instanceof AwsDsqlRetryExhaustedError) {
      return reply(c, { error: "write conflicted with concurrent transactions; please retry" }, 409);
    }
    if (isUnavailable(err)) {
      console.error(JSON.stringify({ level: "error", msg: "database unavailable", error: rootMessage(err) }));
      return reply(c, { error: `database unavailable: ${rootMessage(err)}` }, 503);
    }
    console.error(
      JSON.stringify({ level: "error", msg: "unhandled error", path: c.req.path, error: rootMessage(err), stack: err.stack }),
    );
    return reply(c, { error: "internal error" }, 500);
  });

  return app;
}

export type App = ReturnType<typeof createApp>;
