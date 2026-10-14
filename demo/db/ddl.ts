// The demo schema, statement by statement. DSQL runs one DDL statement per
// transaction and never mixes DDL with DML, so each runs on its own.

export interface TableDdl {
  name: string;
  sql: string;
  /** Used only if `sql` is rejected as unsupported (see setup.ts). */
  fallbackSql?: string;
}

export const TABLES: TableDdl[] = [
  {
    name: "users",
    sql: `CREATE TABLE IF NOT EXISTS users (
  id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  username text NOT NULL UNIQUE,
  display_name text NOT NULL,
  created_at timestamptz NOT NULL DEFAULT now()
)`,
  },
  {
    name: "posts",
    sql: `CREATE TABLE IF NOT EXISTS posts (
  id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  author_id uuid NOT NULL REFERENCES users(id),
  slug text NOT NULL UNIQUE,
  title text NOT NULL,
  body text NOT NULL,
  metadata jsonb NOT NULL DEFAULT '{}'::jsonb,
  created_at timestamptz NOT NULL DEFAULT now()
)`,
    // Unverified on DSQL: a jsonb DEFAULT. The app always supplies metadata, so
    // dropping the default loses nothing.
    fallbackSql: `CREATE TABLE IF NOT EXISTS posts (
  id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  author_id uuid NOT NULL REFERENCES users(id),
  slug text NOT NULL UNIQUE,
  title text NOT NULL,
  body text NOT NULL,
  metadata jsonb NOT NULL,
  created_at timestamptz NOT NULL DEFAULT now()
)`,
  },
  {
    name: "comments",
    sql: `CREATE TABLE IF NOT EXISTS comments (
  id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  post_id uuid NOT NULL REFERENCES posts(id) ON DELETE CASCADE,
  user_id uuid NOT NULL REFERENCES users(id),
  body text NOT NULL,
  created_at timestamptz NOT NULL DEFAULT now()
)`,
  },
];

export interface IndexDdl {
  name: string;
  table: string;
  /** Column list / expressions, e.g. `((metadata->>'category'), created_at)` */
  on: string;
}

export const INDEXES: IndexDdl[] = [
  { name: "posts_created_at_idx", table: "posts", on: "(created_at)" },
  // Same expression the API filters on: posts.metadata->>'category'.
  { name: "posts_category_idx", table: "posts", on: "((metadata->>'category'), created_at)" },
  { name: "comments_post_idx", table: "comments", on: "(post_id, created_at)" },
];

export const createIndexSql = (ix: IndexDdl, ifNotExists = true) =>
  `CREATE INDEX ASYNC ${ifNotExists ? "IF NOT EXISTS " : ""}${ix.name} ON ${ix.table} ${ix.on}`;

export const APP_TABLES = TABLES.map((t) => t.name);
