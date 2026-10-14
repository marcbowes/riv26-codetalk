// Drizzle table definitions. These mirror the DDL in demo/db/setup.ts (and the
// spec): all DDL is applied by hand / by setup.ts, never by drizzle-kit.
import { sql } from "drizzle-orm";
import { type AnyPgColumn, index, jsonb, pgTable, text, timestamp, uuid } from "drizzle-orm/pg-core";

import type { PostMeta } from "./types.js";

export type { PostMeta };

// created_at columns use mode "string": values stay exact (microseconds) and
// are converted to ISO-8601 by pgTimestampToIso (see time.ts).

export const users = pgTable("users", {
  id: uuid("id").primaryKey().defaultRandom(),
  username: text("username").notNull().unique(),
  displayName: text("display_name").notNull(),
  createdAt: timestamp("created_at", { withTimezone: true, mode: "string" }).notNull().defaultNow(),
});

export const posts = pgTable(
  "posts",
  {
    id: uuid("id").primaryKey().defaultRandom(),
    authorId: uuid("author_id")
      .notNull()
      .references(() => users.id),
    slug: text("slug").notNull().unique(),
    title: text("title").notNull(),
    body: text("body").notNull(),
    metadata: jsonb("metadata").$type<PostMeta>().notNull().default(sql`'{}'::jsonb`),
    createdAt: timestamp("created_at", { withTimezone: true, mode: "string" }).notNull().defaultNow(),
  },
  (t) => [
    index("posts_created_at_idx").on(t.createdAt),
    index("posts_category_idx").on(postCategory(t.metadata), t.createdAt),
  ],
);

export const comments = pgTable(
  "comments",
  {
    id: uuid("id").primaryKey().defaultRandom(),
    postId: uuid("post_id")
      .notNull()
      .references(() => posts.id, { onDelete: "cascade" }),
    userId: uuid("user_id")
      .notNull()
      .references(() => users.id),
    body: text("body").notNull(),
    createdAt: timestamp("created_at", { withTimezone: true, mode: "string" }).notNull().defaultNow(),
  },
  (t) => [index("comments_post_idx").on(t.postId, t.createdAt)],
);

/**
 * `metadata->>'category'`: the exact expression posts_category_idx is built on.
 * Filter with this (not `metadata->'category'`, which returns jsonb and can't
 * use the index) so the planner can pick the index.
 */
export function postCategory(metadata: AnyPgColumn) {
  return sql<string>`(${metadata} ->> 'category')`;
}
