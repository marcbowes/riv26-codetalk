// Drizzle table definitions. These mirror the DDL in demo/db/ddl.ts: all DDL
// is applied by setup.ts, never by drizzle-kit. Indexes live only in ddl.ts
// (they're covering, with INCLUDE, which Drizzle's index builder can't express).
import { sql } from "drizzle-orm";
import { type AnyPgColumn, jsonb, pgTable, primaryKey, text, timestamp, uuid } from "drizzle-orm/pg-core";

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

export const posts = pgTable("posts", {
  id: uuid("id").primaryKey().defaultRandom(),
  authorId: uuid("author_id")
    .notNull()
    .references(() => users.id),
  slug: text("slug").notNull().unique(),
  title: text("title").notNull(),
  body: text("body").notNull(),
  metadata: jsonb("metadata").$type<PostMeta>().notNull().default(sql`'{}'::jsonb`),
  createdAt: timestamp("created_at", { withTimezone: true, mode: "string" }).notNull().defaultNow(),
});

// Keyed (post_id, comment_id): a post's comments are one primary-key range.
export const comments = pgTable(
  "comments",
  {
    postId: uuid("post_id")
      .notNull()
      .references(() => posts.id, { onDelete: "cascade" }),
    commentId: uuid("comment_id").notNull().defaultRandom(),
    userId: uuid("user_id")
      .notNull()
      .references(() => users.id),
    body: text("body").notNull(),
    createdAt: timestamp("created_at", { withTimezone: true, mode: "string" }).notNull().defaultNow(),
  },
  (t) => [primaryKey({ columns: [t.postId, t.commentId] })],
);

/**
 * `metadata->>'category'`: the exact expression posts_category_idx is built on.
 * Filter with this (not `metadata->'category'`, which returns jsonb and can't
 * use the index) so the planner can pick the index.
 */
export function postCategory(metadata: AnyPgColumn) {
  return sql<string>`(${metadata} ->> 'category')`;
}
