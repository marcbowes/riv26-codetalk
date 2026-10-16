// Deterministic seed data: the same seed gives the same users, posts and
// comments (ids included), which makes rehearsals repeatable and lets
// `--resume` finish an interrupted seed with ON CONFLICT DO NOTHING.
import { CATEGORIES, type Category } from "@dat401/api/categories";
import type { PostMeta } from "@dat401/api/types";
import { buildPostMeta, slugify } from "@dat401/api/validation";
import { COMMENTS, FIRST_NAMES, HEADINGS, LAST_NAMES, SENTENCES, TAGS, TITLE_TEMPLATES, TOPICS } from "./words.js";

export type Rng = () => number;

/** mulberry32: tiny, fast, good enough for fake data. */
export function rng(seed: number): Rng {
  let a = seed >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) | 0;
    let t = Math.imul(a ^ (a >>> 15), 1 | a);
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

export const int = (r: Rng, min: number, max: number) => min + Math.floor(r() * (max - min + 1));
export const pick = <T>(r: Rng, xs: readonly T[]): T => xs[Math.floor(r() * xs.length)] as T;

export function sample<T>(r: Rng, xs: readonly T[], k: number): T[] {
  const copy = [...xs];
  for (let i = copy.length - 1; i > 0; i--) {
    const j = Math.floor(r() * (i + 1));
    [copy[i], copy[j]] = [copy[j] as T, copy[i] as T];
  }
  return copy.slice(0, k);
}

/** RFC 4122 v4-shaped uuid from the PRNG. */
export function uuid(r: Rng): string {
  const b = Array.from({ length: 16 }, () => Math.floor(r() * 256));
  b[6] = ((b[6] as number) & 0x0f) | 0x40;
  b[8] = ((b[8] as number) & 0x3f) | 0x80;
  const h = b.map((x) => x.toString(16).padStart(2, "0")).join("");
  return `${h.slice(0, 8)}-${h.slice(8, 12)}-${h.slice(12, 16)}-${h.slice(16, 20)}-${h.slice(20)}`;
}

const DAY = 86_400_000;
/** Posts span about two years. */
const POST_SPAN_DAYS = 730;

/**
 * Hands out millisecond timestamps that are never reused (nudging a collision
 * back by 1 ms), so every seeded row has its own created_at. Clients page with
 * `?before=<createdAt>`, and ties at a page boundary would skip rows.
 */
export class DistinctTimes {
  private readonly used = new Set<number>();
  take(ms: number): string {
    let t = Math.floor(ms);
    while (this.used.has(t)) t -= 1;
    this.used.add(t);
    return new Date(t).toISOString();
  }
}
const cap = (s: string) => s.charAt(0).toUpperCase() + s.slice(1);

function fill(template: string, topic: string, topic2: string, n?: number): string {
  return template
    .replaceAll("{Topic}", cap(topic))
    .replaceAll("{topic}", topic)
    .replaceAll("{topic2}", topic2)
    .replaceAll("{n}", String(n ?? 5));
}

// ---------------------------------------------------------------------------

export interface SeedUser {
  id: string;
  username: string;
  displayName: string;
  /** ISO-8601 */
  createdAt: string;
}

export function generateUsers(count: number, seed: number, now: Date): SeedUser[] {
  const r = rng(seed);
  const times = new DistinctTimes();
  const taken = new Set<string>();
  const out: SeedUser[] = [];
  while (out.length < count) {
    const first = pick(r, FIRST_NAMES);
    const last = pick(r, LAST_NAMES);
    const f = first.toLowerCase();
    const l = last.toLowerCase().replace(/[^a-z]/g, "");
    const patterns = [`${f}.${l}`, `${f}${l.charAt(0)}`, `${f.charAt(0)}${l}`, `${f}_${l}`, `${l}${f.charAt(0)}`];
    let username = pick(r, patterns);
    while (taken.has(username)) username = `${pick(r, patterns)}${int(r, 2, 99)}`;
    taken.add(username);
    out.push({
      id: uuid(r),
      username,
      displayName: `${first} ${last}`,
      // Members joined before the first post.
      createdAt: times.take(now.getTime() - (POST_SPAN_DAYS + 7 + r() * 365) * DAY),
    });
  }
  return out;
}

export interface SeedPost {
  id: string;
  authorId: string;
  slug: string;
  title: string;
  body: string;
  metadata: PostMeta;
  /** ISO-8601 */
  createdAt: string;
}

function topicsFor(r: Rng, category: Category): [string, string] {
  const own = TOPICS[category];
  const topic = pick(r, own);
  // Mostly stay on-topic; sometimes borrow from a neighbouring category.
  const pool = r() < 0.7 ? own : TOPICS[pick(r, CATEGORIES)];
  let topic2 = pick(r, pool);
  for (let i = 0; i < 5 && topic2 === topic; i++) topic2 = pick(r, pool);
  return [topic, topic2];
}

function paragraph(r: Rng, topic: string, topic2: string): string {
  return sample(r, SENTENCES, int(r, 3, 6))
    .map((s) => fill(s, topic, topic2))
    .join(" ");
}

export function generateBody(r: Rng, topic: string, topic2: string): string {
  const parts: string[] = [];
  const headings = sample(r, HEADINGS, HEADINGS.length);
  const paragraphs = int(r, 8, 16);
  for (let i = 0; i < paragraphs; i++) {
    if (i > 0 && i % int(r, 2, 3) === 0 && headings.length) parts.push(`## ${headings.shift()}`);
    parts.push(paragraph(r, topic, topic2));
  }
  return parts.join("\n\n");
}

export function generatePosts(authorIds: readonly string[], count: number, seed: number, now: Date): SeedPost[] {
  if (authorIds.length === 0) throw new Error("need at least one user to author posts");
  const r = rng(seed);
  // Skew authorship: a handful of prolific writers, a long tail of occasional ones.
  const authors = sample(r, authorIds, authorIds.length);
  const times = new DistinctTimes();
  const out: SeedPost[] = [];
  for (let i = 0; i < count; i++) {
    const category = pick(r, CATEGORIES);
    const [topic, topic2] = topicsFor(r, category);
    const title = fill(pick(r, TITLE_TEMPLATES), topic, topic2, pick(r, [3, 4, 5, 7, 10]));
    const body = generateBody(r, topic, topic2);
    const tags = sample(r, TAGS[category], int(r, 2, 4));
    const suffix = uuid(r).slice(0, 8);
    out.push({
      id: uuid(r),
      authorId: authors[Math.floor(authors.length * r() ** 2.2)] as string,
      slug: slugify(title, suffix),
      title,
      body,
      metadata: buildPostMeta(body, category, tags),
      // Denser towards the present, spread over about two years.
      createdAt: times.take(now.getTime() - 3_600_000 - POST_SPAN_DAYS * DAY * r() ** 1.3),
    });
  }
  return out;
}

export interface SeedComment {
  commentId: string;
  postId: string;
  userId: string;
  body: string;
  /** ISO-8601 */
  createdAt: string;
}

export function generateComments(
  postsIn: readonly { id: string; createdAt: Date | string }[],
  userIds: readonly string[],
  maxPerPost: number,
  seed: number,
  now: Date,
): SeedComment[] {
  if (userIds.length === 0) throw new Error("need at least one user to comment");
  const r = rng(seed);
  const allTopics = Object.values(TOPICS).flat();
  const times = new DistinctTimes();
  const out: SeedComment[] = [];
  for (const post of postsIn) {
    const n = Math.floor(r() ** 1.4 * (maxPerPost + 1));
    const start = new Date(post.createdAt).getTime() + 5 * 60_000;
    const window = Math.max(60_000, Math.min(30 * DAY, now.getTime() - start));
    for (let i = 0; i < n; i++) {
      const parts = sample(r, COMMENTS, r() < 0.3 ? 2 : 1).map((c) => fill(c, pick(r, allTopics), pick(r, allTopics)));
      out.push({
        commentId: uuid(r),
        postId: post.id,
        userId: pick(r, userIds),
        body: parts.join(" "),
        createdAt: times.take(start + r() * window),
      });
    }
  }
  return out;
}

export function chunk<T>(xs: readonly T[], size: number): T[][] {
  const out: T[][] = [];
  for (let i = 0; i < xs.length; i += size) out.push(xs.slice(i, i + size));
  return out;
}

/** Run `fn` over batches with at most `concurrency` in flight, printing progress. */
export async function runBatches<T>(
  label: string,
  batches: readonly T[][],
  concurrency: number,
  fn: (batch: T[]) => Promise<unknown>,
): Promise<void> {
  const started = performance.now();
  const total = batches.reduce((n, b) => n + b.length, 0);
  let next = 0;
  let done = 0;
  let rows = 0;
  const report = (final = false) => {
    const secs = (performance.now() - started) / 1000;
    const line = `  ${label.padEnd(9)} ${String(done).padStart(4)}/${batches.length} batches  ${String(rows).padStart(6)}/${total} rows  ${secs.toFixed(1)}s  (${Math.round(rows / Math.max(secs, 0.001))} rows/s)`;
    if (process.stdout.isTTY && !final) process.stdout.write(`\r${line}`);
    else if (final) process.stdout.write(`${process.stdout.isTTY ? "\r" : ""}${line}\n`);
  };
  const worker = async () => {
    while (next < batches.length) {
      const batch = batches[next++] as T[];
      await fn(batch);
      done++;
      rows += batch.length;
      report();
    }
  };
  await Promise.all(Array.from({ length: Math.min(concurrency, batches.length) }, worker));
  report(true);
}
