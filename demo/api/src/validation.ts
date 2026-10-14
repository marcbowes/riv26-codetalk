// Request parsing and validation. Pure functions (no DB) so they're unit-tested
// in test/validation.test.ts and run before any connection is opened.
import { CATEGORIES, type Category, isCategory } from "./categories.js";
import type { PostMeta } from "./types.js";

export type HttpStatus = 400 | 404 | 409 | 413 | 429 | 500 | 503;

export class HttpError extends Error {
  constructor(
    readonly status: HttpStatus,
    message: string,
    readonly details?: string[],
  ) {
    super(message);
    this.name = "HttpError";
  }
}

const UUID_RE = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

export function isUuid(value: unknown): value is string {
  return typeof value === "string" && UUID_RE.test(value);
}

export const LIMITS = {
  titleMax: 200,
  bodyMax: 50_000,
  commentMax: 2_000,
  tagsMax: 8,
  tagMax: 32,
} as const;

/** `?limit=` → integer in [1, max]; absent → fallback; anything else → 400. */
export function parseLimit(raw: string | undefined, fallback: number, max: number): number {
  if (raw === undefined || raw === "") return fallback;
  if (!/^\d+$/.test(raw)) throw new HttpError(400, `limit must be an integer between 1 and ${max}`);
  const n = Number(raw);
  if (n < 1 || n > max) throw new HttpError(400, `limit must be an integer between 1 and ${max}`);
  return n;
}

const ISO_WITH_ZONE = /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}(:\d{2}(\.\d{1,9})?)?(Z|[+-]\d{2}:?\d{2})$/;

/**
 * `?before=` (cursor: posts older than this). An ISO-8601 timestamp with a zone,
 * e.g. a `createdAt` from a previous page. Returned unchanged so the database
 * compares at full (microsecond) precision.
 */
export function parseBefore(raw: string | undefined): string | undefined {
  if (raw === undefined || raw === "") return undefined;
  if (!ISO_WITH_ZONE.test(raw) || Number.isNaN(Date.parse(raw))) {
    throw new HttpError(400, "before must be an ISO-8601 timestamp with a zone, e.g. 2026-12-01T17:00:00.000Z");
  }
  return raw;
}

export function parseCategory(raw: string | undefined): Category | undefined {
  if (raw === undefined || raw === "") return undefined;
  if (!isCategory(raw)) throw new HttpError(400, `unknown category; expected one of: ${CATEGORIES.join(", ")}`);
  return raw;
}

export interface NewPost {
  authorId: string;
  title: string;
  body: string;
  category: Category;
  tags: string[];
}

export interface NewComment {
  userId: string;
  body: string;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function requiredText(
  obj: Record<string, unknown>,
  key: string,
  max: number,
  problems: string[],
): string {
  const v = obj[key];
  if (typeof v !== "string" || v.trim() === "") {
    problems.push(`${key} is required`);
    return "";
  }
  const trimmed = v.trim();
  if (trimmed.length > max) problems.push(`${key} must be at most ${max} characters`);
  return trimmed;
}

/** Lowercase, hyphenate, dedupe. Returns undefined if `raw` isn't a string array. */
export function normalizeTags(raw: unknown): string[] | undefined {
  if (raw === undefined || raw === null) return [];
  if (!Array.isArray(raw) || !raw.every((t) => typeof t === "string")) return undefined;
  const out: string[] = [];
  for (const t of raw as string[]) {
    const tag = t
      .toLowerCase()
      .trim()
      .replace(/[^a-z0-9]+/g, "-")
      .replace(/^-+|-+$/g, "")
      .slice(0, LIMITS.tagMax);
    if (tag && !out.includes(tag)) out.push(tag);
  }
  return out;
}

export function validateNewPost(input: unknown): NewPost {
  if (!isRecord(input)) throw new HttpError(400, "body must be a JSON object");
  const problems: string[] = [];
  const authorId = input.authorId;
  if (!isUuid(authorId)) problems.push("authorId must be a uuid");
  const title = requiredText(input, "title", LIMITS.titleMax, problems);
  const body = requiredText(input, "body", LIMITS.bodyMax, problems);
  const category = input.category;
  if (!isCategory(category)) problems.push(`category must be one of: ${CATEGORIES.join(", ")}`);
  const tags = normalizeTags(input.tags);
  if (tags === undefined) problems.push("tags must be an array of strings");
  else if (tags.length > LIMITS.tagsMax) problems.push(`at most ${LIMITS.tagsMax} tags`);
  if (problems.length > 0) throw new HttpError(400, problems.join("; "), problems);
  return {
    authorId: authorId as string,
    title,
    body,
    category: category as Category,
    tags: tags ?? [],
  };
}

export function validateNewComment(input: unknown): NewComment {
  if (!isRecord(input)) throw new HttpError(400, "body must be a JSON object");
  const problems: string[] = [];
  const userId = input.userId;
  if (!isUuid(userId)) problems.push("userId must be a uuid");
  const body = requiredText(input, "body", LIMITS.commentMax, problems);
  if (problems.length > 0) throw new HttpError(400, problems.join("; "), problems);
  return { userId: userId as string, body };
}

// ---------------------------------------------------------------------------
// Text helpers shared by the API (new posts) and the seed script.

/** Collapse whitespace and drop markdown heading markers. */
function plain(text: string): string {
  return text
    .replace(/^#+\s*/gm, "")
    .replace(/\s+/g, " ")
    .trim();
}

/** Cut at a word boundary, adding an ellipsis if anything was dropped. */
export function truncate(text: string, max: number): string {
  const s = plain(text);
  if (s.length <= max) return s;
  const cut = s.slice(0, max + 1);
  const space = cut.lastIndexOf(" ");
  return `${(space > max * 0.6 ? cut.slice(0, space) : s.slice(0, max)).replace(/[\s,.;:!?-]+$/, "")}…`;
}

export const EXCERPT_CHARS = 220;
/** How much of the body the list query pulls back to build an excerpt. */
export const EXCERPT_SOURCE_CHARS = 400;

export function excerpt(body: string): string {
  return truncate(body, EXCERPT_CHARS);
}

export function slugify(title: string, suffix: string): string {
  const base = title
    .toLowerCase()
    .normalize("NFKD")
    .replace(/[\u0300-\u036f]/g, "")
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "")
    .slice(0, 72)
    .replace(/-+$/, "");
  return `${base || "post"}-${suffix}`;
}

/** Short random suffix that keeps slugs unique without a lookup. */
export function slugSuffix(): string {
  return crypto.randomUUID().replace(/-/g, "").slice(0, 8);
}

export function readingMinutes(body: string): number {
  const words = plain(body).split(" ").filter(Boolean).length;
  return Math.max(1, Math.round(words / 220));
}

export function buildPostMeta(body: string, category: string, tags: string[]): PostMeta {
  return {
    category,
    tags,
    readingMinutes: readingMinutes(body),
    seo: { description: truncate(body, 155) },
  };
}
