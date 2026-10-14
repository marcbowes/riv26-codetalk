// The blog's fixed set of categories. Stored in posts.metadata->>'category' and
// indexed by posts_category_idx. Shared with the seed script (demo/db).
export const CATEGORIES = [
  "databases",
  "distributed-systems",
  "serverless",
  "performance",
  "security",
  "devops",
  "frontend",
  "career",
] as const;

export type Category = (typeof CATEGORIES)[number];

export function isCategory(value: unknown): value is Category {
  return typeof value === "string" && (CATEGORIES as readonly string[]).includes(value);
}
