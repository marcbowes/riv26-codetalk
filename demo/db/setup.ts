// Idempotent database setup for the demo cluster. Run as `admin`, once, against
// REGION_A: peered clusters are one logical database, so the schema, roles,
// IAM mappings and data all show up in REGION_B too.
//
//   npx tsx db/setup.ts --endpoint <id>.dsql.us-east-1.on.aws \
//     --role-arn arn:aws:iam::123456789012:role/dat401-blog-api-us-east-1 \
//     --role-arn arn:aws:iam::123456789012:role/dat401-blog-api-us-east-2
//
// Safe to re-run: every step checks before it acts.
import { parseArgs } from "node:util";
import { AuroraDSQLPool } from "@aws/aurora-dsql-node-postgres-connector";
import {
  ensureIamGrants,
  ensureIndexes,
  ensureRole,
  ensureTables,
  errCode,
  errMessage,
  querier,
  seedAll,
  summary,
  type SeedOptions,
} from "./steps.js";

const HELP = `Usage: tsx db/setup.ts --endpoint <host> [options]

  --endpoint, -e <host>   Cluster endpoint (default: $CLUSTER_ENDPOINT)
  --region <region>       AWS Region (default: parsed from the endpoint)
  --role-arn <arn>        IAM role to map to the app role (repeatable)
  --app-role <name>       Database role for the app (default: blog_app)
  --regrant               AWS IAM REVOKE + GRANT even if a mapping exists
                          (use after recreating an IAM role with the same name)
  --skip-seed             Schema, roles and grants only
  --resume                Re-run the (deterministic) seed with ON CONFLICT DO
                          NOTHING, to finish an interrupted seed
  --users <n>             Seed users (default: 500)
  --posts <n>             Seed posts (default: 5000)
  --max-comments <n>      Max comments per post (default: 30)
  --concurrency <n>       Concurrent seed batches (default: 8)
  --seed <n>              PRNG seed (default: 401)
`;

interface Options extends SeedOptions {
  endpoint: string;
  region: string | undefined;
  roleArns: string[];
  appRole: string;
  regrant: boolean;
  seed: boolean;
}

function parseOptions(): Options {
  const { values } = parseArgs({
    options: {
      endpoint: { type: "string", short: "e" },
      region: { type: "string" },
      "role-arn": { type: "string", multiple: true },
      "app-role": { type: "string" },
      regrant: { type: "boolean" },
      "skip-seed": { type: "boolean" },
      resume: { type: "boolean" },
      users: { type: "string" },
      posts: { type: "string" },
      "max-comments": { type: "string" },
      concurrency: { type: "string" },
      seed: { type: "string" },
      help: { type: "boolean", short: "h" },
    },
  });
  if (values.help) {
    console.log(HELP);
    process.exit(0);
  }
  const endpoint = values.endpoint ?? process.env.CLUSTER_ENDPOINT;
  if (!endpoint) fail("missing --endpoint (or CLUSTER_ENDPOINT)\n\n" + HELP);
  const num = (name: string, raw: string | undefined, fallback: number, min = 0) => {
    if (raw === undefined) return fallback;
    const n = Number(raw);
    if (!Number.isInteger(n) || n < min) fail(`--${name} must be an integer >= ${min}`);
    return n;
  };
  const appRole = values["app-role"] ?? "blog_app";
  if (!/^[a-z_][a-z0-9_]{0,62}$/.test(appRole)) fail("--app-role must be a lowercase identifier");
  return {
    endpoint,
    region: values.region,
    roleArns: values["role-arn"] ?? [],
    appRole,
    regrant: values.regrant ?? false,
    seed: !(values["skip-seed"] ?? false),
    resume: values.resume ?? false,
    users: num("users", values.users, 500, 1),
    posts: num("posts", values.posts, 5000),
    maxComments: num("max-comments", values["max-comments"], 30),
    concurrency: num("concurrency", values.concurrency, 8, 1),
    seedValue: num("seed", values.seed, 401),
  };
}

function fail(message: string): never {
  console.error(`error: ${message}`);
  process.exit(1);
}

const log = (msg: string) => console.log(msg);
const step = (title: string) => console.log(`\n== ${title}`);

async function main(): Promise<void> {
  const o = parseOptions();
  const pool = new AuroraDSQLPool({
    host: o.endpoint,
    user: "admin",
    ...(o.region ? { region: o.region } : {}),
    max: o.concurrency + 1,
    connectionTimeoutMillis: 15_000,
    application_name: "dat401-setup",
  });
  pool.on("error", (err) => console.warn(`idle connection error: ${err.message}`));
  const q = querier(pool);
  try {
    step(`connect ${o.endpoint}`);
    const who = await q<{ user: string; version: string }>(
      `SELECT current_user AS user, current_setting('server_version') AS version`,
    );
    log(`  connected as ${who.rows[0]?.user} (PostgreSQL ${who.rows[0]?.version})`);
    await ensureTables(q);
    await ensureIndexes(q);
    await ensureRole(q, o.appRole);
    await ensureIamGrants(q, o.appRole, o.roleArns, o.regrant);
    if (o.seed) await seedAll(pool, q, o);
    await summary(q, o.appRole);
    log("\nsetup complete");
  } finally {
    await pool.end();
  }
}

main().catch((err) => {
  console.error(`\nsetup failed: ${errMessage(err)}${errCode(err) ? ` (${errCode(err)})` : ""}`);
  process.exit(1);
});
