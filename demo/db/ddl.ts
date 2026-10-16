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
    // Keyed by post first: the table is stored by its primary key, so a post's
    // comments sit together and "comments for a post" is a primary-key range
    // scan, with no secondary index (the talk's step 1 and step 5).
    sql: `CREATE TABLE IF NOT EXISTS comments (
  post_id uuid NOT NULL REFERENCES posts(id) ON DELETE CASCADE,
  comment_id uuid NOT NULL DEFAULT gen_random_uuid(),
  user_id uuid NOT NULL REFERENCES users(id),
  body text NOT NULL,
  created_at timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (post_id, comment_id)
)`,
  },
];

export interface IndexDdl {
  name: string;
  table: string;
  /** Column list / expressions, e.g. `((metadata->>'category'), created_at)` */
  on: string;
}

// Both list indexes are covering: they INCLUDE every column the list query
// reads (the excerpt comes from metadata, not body). In DSQL the table is its
// primary-key B-tree, so an index that doesn't cover the query costs a lookup
// per row, and the planner would rather scan the table (the talk's step 6).
// `id` is listed explicitly: selecting the primary key from a secondary index
// isn't index-only in DSQL unless it's INCLUDEd (measured: Index Scan 20 ms ->
// Index Only Scan 8 ms for the home page).
const LIST_COLUMNS = "INCLUDE (id, author_id, slug, title, metadata)";

export const INDEXES: IndexDdl[] = [
  { name: "posts_recent_idx", table: "posts", on: `(created_at) ${LIST_COLUMNS}` },
  // Same expression the API filters on: posts.metadata->>'category'.
  { name: "posts_category_idx", table: "posts", on: `((metadata->>'category'), created_at) ${LIST_COLUMNS}` },
];

export const createIndexSql = (ix: IndexDdl, ifNotExists = true) =>
  `CREATE INDEX ASYNC ${ifNotExists ? "IF NOT EXISTS " : ""}${ix.name} ON ${ix.table} ${ix.on}`;

export const APP_TABLES = TABLES.map((t) => t.name);
