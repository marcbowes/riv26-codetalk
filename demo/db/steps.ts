// The setup steps, each idempotent. Used by setup.ts (the CLI).
import { drizzle, withRetry, type AwsDsqlRetryConfig } from "@aws/aurora-dsql-drizzle";
import * as schema from "@dat401/api/schema";
import type { Pool, QueryResult, QueryResultRow } from "pg";
import { APP_TABLES, INDEXES, TABLES, createIndexSql, type IndexDdl } from "./ddl.js";
import { chunk, generateComments, generatePosts, generateUsers, runBatches } from "./seed.js";

// DSQL limits a transaction to 3,000 modified rows and 10 MiB. Stay well under.
export const BATCH = { users: 500, posts: 250, comments: 1000 } as const;

export const RETRY: AwsDsqlRetryConfig = { maxRetries: 10, baseDelayMs: 100, maxDelayMs: 3_000 };

export interface SeedOptions {
  /** Insert with ON CONFLICT DO NOTHING even into non-empty tables. */
  resume: boolean;
  users: number;
  posts: number;
  maxComments: number;
  concurrency: number;
  seedValue: number;
}

const log = (msg: string) => console.log(msg);
const step = (title: string) => console.log(`\n== ${title}`);

export function errCode(err: unknown): string | undefined {
  for (let e: any = err; e && typeof e === "object"; e = e.cause) if (typeof e.code === "string") return e.code;
  return undefined;
}

export function errMessage(err: unknown): string {
  let msg = String(err);
  for (let e: any = err; e && typeof e === "object"; e = e.cause) if (e.message) msg = e.message;
  return msg;
}

/** pool.query with OCC retry (DDL elsewhere can make cached catalogs stale: OC001). */
export function querier(pool: Pool) {
  return <R extends QueryResultRow = any>(text: string, params?: unknown[]): Promise<QueryResult<R>> =>
    withRetry(() => pool.query<R>(text, params), RETRY);
}
export type Q = ReturnType<typeof querier>;

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

// ---------------------------------------------------------------------------

async function tableExists(q: Q, name: string): Promise<boolean> {
  const r = await q(
    `SELECT 1 FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace
     WHERE n.nspname = 'public' AND c.relname = $1 AND c.relkind = 'r'`,
    [name],
  );
  return (r.rowCount ?? 0) > 0;
}

/** --reset: drop the blog tables (children first). */
export async function resetTables(q: Q): Promise<void> {
  step("reset (--reset)");
  for (const name of [...APP_TABLES].reverse()) {
    await q(`DROP TABLE IF EXISTS ${name}`);
    log(`  ${name}: dropped`);
  }
}

/**
 * Tables from before a schema change don't get altered (CREATE TABLE IF NOT
 * EXISTS skips them), so catch the old shapes and say what to do.
 */
export async function checkTableShapes(q: Q): Promise<void> {
  if (!(await tableExists(q, "comments"))) return;
  const r = await q<{ n: number }>(
    `SELECT count(*)::int AS n FROM information_schema.columns
     WHERE table_schema = 'public' AND table_name = 'comments' AND column_name = 'comment_id'`,
  );
  if ((r.rows[0]?.n ?? 0) === 0) {
    throw new Error(
      "comments has the old shape (keyed by id, not (post_id, comment_id)). " +
        "Re-run with --reset (deploy.sh: SETUP_ARGS=--reset) to drop and reseed the blog tables.",
    );
  }
}

export async function ensureTables(q: Q): Promise<void> {
  step("tables");
  for (const t of TABLES) {
    if (await tableExists(q, t.name)) {
      log(`  ${t.name}: exists`);
      continue;
    }
    try {
      await q(t.sql);
      log(`  ${t.name}: created`);
    } catch (err) {
      const code = errCode(err);
      if (!t.fallbackSql || !(code === "0A000" || code === "42601" || code === "42804")) throw err;
      console.warn(`  ${t.name}: ${errMessage(err)} (${code}); retrying without the jsonb DEFAULT`);
      await q(t.fallbackSql);
      log(`  ${t.name}: created (fallback DDL)`);
    }
  }
}

type IndexState = "missing" | "valid" | "invalid";

async function indexState(q: Q, name: string): Promise<IndexState> {
  const r = await q<{ indisvalid: boolean }>(
    `SELECT i.indisvalid FROM pg_index i
       JOIN pg_class c ON c.oid = i.indexrelid
       JOIN pg_namespace n ON n.oid = c.relnamespace
     WHERE n.nspname = 'public' AND c.relname = $1`,
    [name],
  );
  const row = r.rows[0];
  if (!row) return "missing";
  return row.indisvalid ? "valid" : "invalid";
}

async function activeIndexJob(q: Q, name: string): Promise<string | undefined> {
  const r = await q<{ job_id: string }>(
    `SELECT job_id FROM sys.jobs
     WHERE job_type = 'INDEX_BUILD' AND object_name = $1 AND status IN ('submitted', 'processing')
     ORDER BY start_time DESC LIMIT 1`,
    [`public.${name}`],
  );
  return r.rows[0]?.job_id;
}

async function submitIndex(q: Q, ix: IndexDdl): Promise<string | undefined> {
  let r: QueryResult<{ job_id?: string }>;
  try {
    r = await q(createIndexSql(ix, true));
  } catch (err) {
    // Fallback in case `IF NOT EXISTS` isn't accepted together with ASYNC.
    // Safe: we only get here after checking the index doesn't exist.
    if (errCode(err) !== "42601") throw err;
    console.warn(`  ${ix.name}: IF NOT EXISTS rejected (${errMessage(err)}); retrying without it`);
    r = await q(createIndexSql(ix, false));
  }
  return r.rows[0]?.job_id;
}

async function waitForIndex(q: Q, ix: IndexDdl, jobId: string | undefined): Promise<void> {
  for (let attempt = 0; attempt < 60; attempt++) {
    if (jobId) {
      if (!/^[a-z0-9]+$/.test(jobId)) throw new Error(`unexpected job id ${JSON.stringify(jobId)}`);
      await q(`CALL sys.wait_for_job('${jobId}')`);
    }
    if ((await indexState(q, ix.name)) === "valid") return;
    if (jobId) {
      const r = await q<{ status: string; details: string | null }>(
        `SELECT status, details FROM sys.jobs WHERE job_id = $1`,
        [jobId],
      );
      const job = r.rows[0];
      if (job?.status === "failed") {
        throw new Error(
          `index build ${ix.name} failed: ${job.details ?? "(no details)"}. ` +
            `It is left INVALID; re-run setup to drop and rebuild it.`,
        );
      }
    } else {
      jobId = await activeIndexJob(q, ix.name);
    }
    await sleep(2_000);
  }
  throw new Error(`timed out waiting for index ${ix.name} to become valid`);
}

export async function ensureIndexes(q: Q): Promise<void> {
  step("indexes (CREATE INDEX ASYNC)");
  const pending: { ix: IndexDdl; jobId: string | undefined }[] = [];
  for (const ix of INDEXES) {
    let state = await indexState(q, ix.name);
    if (state === "valid") {
      log(`  ${ix.name}: exists`);
      continue;
    }
    if (state === "invalid") {
      const jobId = await activeIndexJob(q, ix.name);
      if (jobId) {
        log(`  ${ix.name}: build already in progress (job ${jobId})`);
        pending.push({ ix, jobId });
        continue;
      }
      // A failed async build leaves an INVALID index that must be dropped by hand.
      console.warn(`  ${ix.name}: INVALID with no running job; dropping and rebuilding`);
      await q(`DROP INDEX ${ix.name}`);
      state = "missing";
    }
    const jobId = await submitIndex(q, ix);
    log(`  ${ix.name}: submitted${jobId ? ` (job ${jobId})` : ""}`);
    pending.push({ ix, jobId });
  }
  for (const { ix, jobId } of pending) {
    const started = performance.now();
    await waitForIndex(q, ix, jobId);
    log(`  ${ix.name}: valid after ${((performance.now() - started) / 1000).toFixed(1)}s`);
  }
}

export async function ensureRole(q: Q, role: string): Promise<void> {
  step(`role ${role}`);
  const r = await q(`SELECT 1 FROM pg_roles WHERE rolname = $1`, [role]);
  if ((r.rowCount ?? 0) > 0) {
    log(`  ${role}: exists`);
  } else {
    await q(`CREATE ROLE ${role} WITH LOGIN`);
    log(`  ${role}: created`);
  }
  // No GRANT USAGE ON SCHEMA public: DSQL rejects it ("feature not supported on
  // system entity") and table grants are enough.
  await q(`GRANT SELECT, INSERT, UPDATE, DELETE ON ${APP_TABLES.join(", ")} TO ${role}`);
  log(`  granted SELECT, INSERT, UPDATE, DELETE on ${APP_TABLES.join(", ")}`);
}

const IAM_ARN = /^arn:aws[a-z-]*:iam::\d{12}:(role|user)\/[A-Za-z0-9+=,.@_/-]{1,512}$/;

async function mappedRoles(q: Q, arn: string): Promise<string[] | undefined> {
  try {
    const r = await q<{ pg_role_name: string }>(
      `SELECT pg_role_name FROM sys.iam_pg_role_mappings WHERE arn = $1`,
      [arn],
    );
    return r.rows.map((row) => row.pg_role_name);
  } catch (err) {
    console.warn(`  could not read sys.iam_pg_role_mappings (${errMessage(err)})`);
    return undefined;
  }
}

export async function ensureIamGrants(q: Q, role: string, arns: string[], regrant: boolean): Promise<void> {
  step("IAM role mappings");
  if (arns.length === 0) log("  (no --role-arn given)");
  for (const arn of arns) {
    if (!IAM_ARN.test(arn)) throw new Error(`not an IAM role/user ARN: ${arn}`);
    const roles = await mappedRoles(q, arn);
    const mapped = roles?.includes(role) ?? false;
    if (mapped && !regrant) {
      log(`  ${arn} -> ${role}: already mapped`);
      continue;
    }
    if (mapped && regrant) {
      await q(`AWS IAM REVOKE ${role} FROM '${arn}'`);
      log(`  ${arn} -> ${role}: revoked (regrant)`);
    }
    try {
      await q(`AWS IAM GRANT ${role} TO '${arn}'`);
      log(`  ${arn} -> ${role}: granted`);
    } catch (err) {
      // Mapping lookups failed above, or a concurrent run won: treat "already exists" as done.
      if (errCode(err) === "42710" || /already/i.test(errMessage(err))) {
        log(`  ${arn} -> ${role}: already mapped (${errMessage(err)})`);
      } else {
        throw err;
      }
    }
  }
}

async function isEmpty(q: Q, table: string): Promise<boolean> {
  const r = await q(`SELECT 1 FROM ${table} LIMIT 1`);
  return (r.rowCount ?? 0) === 0;
}

export async function seedAll(pool: Pool, q: Q, o: SeedOptions): Promise<void> {
  step(o.resume ? "seed (resume: ON CONFLICT DO NOTHING)" : "seed");
  const db = drizzle({ client: pool, schema });
  const now = new Date();
  let wrote = false;

  if (o.resume || (await isEmpty(q, "users"))) {
    const rows = generateUsers(o.users, o.seedValue, now);
    await runBatches("users", chunk(rows, BATCH.users), o.concurrency, (batch) =>
      withRetry(() => {
        const ins = db.insert(schema.users).values(batch);
        return o.resume ? ins.onConflictDoNothing() : ins;
      }, RETRY),
    );
    wrote = true;
  } else {
    log("  users: not empty, skipping");
  }
  const userIds = (await q<{ id: string }>(`SELECT id FROM users ORDER BY id LIMIT 20000`)).rows.map((r) => r.id);

  if (o.resume || (await isEmpty(q, "posts"))) {
    const rows = generatePosts(userIds, o.posts, o.seedValue + 1, now);
    await runBatches("posts", chunk(rows, BATCH.posts), o.concurrency, (batch) =>
      withRetry(() => {
        const ins = db.insert(schema.posts).values(batch);
        return o.resume ? ins.onConflictDoNothing() : ins;
      }, RETRY),
    );
    wrote = true;
  } else {
    log("  posts: not empty, skipping");
  }

  if (o.resume || (await isEmpty(q, "comments"))) {
    const posts = (
      await q<{ id: string; created_at: Date }>(`SELECT id, created_at FROM posts ORDER BY id LIMIT 20000`)
    ).rows.map((r) => ({ id: r.id, createdAt: new Date(r.created_at) }));
    const rows = generateComments(posts, userIds, o.maxComments, o.seedValue + 2, now);
    await runBatches("comments", chunk(rows, BATCH.comments), o.concurrency, (batch) =>
      withRetry(() => {
        const ins = db.insert(schema.comments).values(batch);
        return o.resume ? ins.onConflictDoNothing() : ins;
      }, RETRY),
    );
    wrote = true;
  } else {
    log("  comments: not empty, skipping");
  }

  if (wrote) {
    // DSQL also auto-analyzes in the background; this just gets fresh stats now.
    for (const t of APP_TABLES) {
      try {
        await q(`ANALYZE ${t}`);
      } catch (err) {
        console.warn(`  ANALYZE ${t} failed (${errMessage(err)}); the background job will catch up`);
      }
    }
    log("  analyzed");
  }
}

export async function summary(q: Q, role: string): Promise<void> {
  step("summary");
  for (const t of APP_TABLES) {
    const r = await q<{ n: number }>(`SELECT count(*)::int AS n FROM ${t}`);
    log(`  ${t.padEnd(9)} ${r.rows[0]?.n ?? 0} rows`);
  }
  for (const ix of INDEXES) log(`  ${ix.name.padEnd(21)} ${await indexState(q, ix.name)}`);
  try {
    const m = await q<{ arn: string; pg_role_name: string }>(
      `SELECT arn, pg_role_name FROM sys.iam_pg_role_mappings WHERE pg_role_name = $1 ORDER BY arn`,
      [role],
    );
    for (const row of m.rows) log(`  ${row.pg_role_name} <- ${row.arn}`);
  } catch {
    // reported earlier
  }
}

