// Database wiring: an AuroraDSQLPool (node-postgres + IAM tokens) handed to the
// Drizzle adapter. We build the pool ourselves (rather than `drizzle({ connection })`)
// so we control pool sizing and connection lifetime.
import { drizzle } from "@aws/aurora-dsql-drizzle";
import { AuroraDSQLPool } from "@aws/aurora-dsql-node-postgres-connector";
import * as schema from "./schema.js";
import { HttpError } from "./validation.js";

export interface DbOptions {
  /** `<cluster-id>.dsql.<region>.on.aws` */
  host: string;
  /** Database role; IAM-mapped with `AWS IAM GRANT`. */
  user: string;
  /** Pool size. 1 on Lambda: one request per environment, one connection each. */
  max: number;
}

export function createDb({ host, user, max }: DbOptions) {
  const pool = new AuroraDSQLPool({
    host,
    user,
    max,
    // DSQL closes connections after 60 minutes. Recycle well before that.
    maxLifetimeSeconds: 50 * 60,
    idleTimeoutMillis: 10 * 60_000,
    // Fail fast (well inside the 10 s Lambda timeout) if the cluster is unreachable.
    connectionTimeoutMillis: 5_000,
    keepAlive: true,
    application_name: "dat401-blog",
  });
  // An idle connection that the server closes emits 'error' on the pool. Without
  // a listener that would crash the process.
  pool.on("error", (err) => {
    console.warn(JSON.stringify({ level: "warn", msg: "idle connection error", error: err.message }));
  });
  return drizzle({ client: pool, schema });
}

export type Db = ReturnType<typeof createDb>;

/**
 * Lazily create the Db from CLUSTER_ENDPOINT / DB_USER on first use, so a
 * missing endpoint turns into a clear 503 instead of a crash at import time.
 */
export function lazyDbFromEnv(max: number): () => Db {
  let db: Db | undefined;
  return () => {
    if (db) return db;
    const host = process.env.CLUSTER_ENDPOINT?.trim();
    if (!host) throw new HttpError(503, "database not configured: set CLUSTER_ENDPOINT");
    const user = process.env.DB_USER?.trim() || "blog_app";
    db = createDb({ host, user, max });
    return db;
  };
}

// ---------------------------------------------------------------------------
// Error classification. Drizzle 0.45 wraps driver errors in DrizzleQueryError
// (original on `.cause`), so walk the chain.

interface ErrorLike {
  code?: unknown;
  message?: unknown;
  cause?: unknown;
  name?: unknown;
}

function* chain(err: unknown): Generator<ErrorLike> {
  let cur: unknown = err;
  for (let i = 0; i < 8 && cur && typeof cur === "object"; i++) {
    yield cur as ErrorLike;
    cur = (cur as ErrorLike).cause;
  }
}

function codeOf(err: unknown): string | undefined {
  for (const e of chain(err)) if (typeof e.code === "string") return e.code;
  return undefined;
}

function messages(err: unknown): string {
  return [...chain(err)].map((e) => String(e.message ?? "")).join(" | ");
}

/** The most specific (innermost) message in the cause chain. */
export function rootMessage(err: unknown): string {
  const all = [...chain(err)];
  for (let i = all.length - 1; i >= 0; i--) {
    const m = all[i]?.message;
    if (typeof m === "string" && m) return m;
  }
  return String(err);
}

/**
 * The server dropped an established connection (DSQL's 60-minute limit, a frozen
 * Lambda environment thawing with a dead socket). Safe to retry once on a fresh
 * connection.
 */
export function isBrokenConnection(err: unknown): boolean {
  const code = codeOf(err);
  if (code && ["ECONNRESET", "EPIPE", "57P01", "57P02", "57P03", "08003", "08006"].includes(code)) return true;
  return /Connection terminated|connection is closed|Client has encountered a connection error/i.test(
    messages(err),
  );
}

/**
 * DSQL refused a connection: 53400 CONFIGURED_LIMIT_EXCEEDED (more than 100 new
 * connections/s per cluster, after the burst) or 53300 TOO_MANY_CONNECTIONS.
 * The API answers 429 so clients back off and retry.
 */
export function isConnectionLimited(err: unknown): boolean {
  const code = codeOf(err);
  if (code === "53400" || code === "53300") return true;
  return /Unable to accept connection, (rate exceeded|too many open connections)/i.test(messages(err));
}

/** Errors that mean "the database is unavailable to us" → 503. */
export function isUnavailable(err: unknown): boolean {
  if (isBrokenConnection(err)) return true;
  const code = codeOf(err);
  if (code) {
    if (/^(08|57P|28|53[34])/.test(code)) return true; // connection, shutdown, auth, too many connections / rate
    if (["ENOTFOUND", "ECONNREFUSED", "ETIMEDOUT", "EAI_AGAIN", "EHOSTUNREACH", "ENETUNREACH"].includes(code)) {
      return true;
    }
  }
  return /timeout exceeded when trying to connect|Failed to generate DSQL token|password authentication failed|Unable to parse region/i.test(
    messages(err),
  );
}

export function isUniqueViolation(err: unknown): boolean {
  return codeOf(err) === "23505";
}

export function isForeignKeyViolation(err: unknown): boolean {
  return codeOf(err) === "23503";
}

export async function retryBrokenConnectionOnce<T>(fn: () => Promise<T>): Promise<T> {
  try {
    return await fn();
  } catch (err) {
    if (!isBrokenConnection(err)) throw err;
    console.warn(JSON.stringify({ level: "warn", msg: "retrying on a fresh connection", error: rootMessage(err) }));
    return fn();
  }
}
