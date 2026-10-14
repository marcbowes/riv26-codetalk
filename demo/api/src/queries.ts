// The blog's queries, in Drizzle. Each function issues its statements one after
// another (never in parallel): on Lambda the pool has exactly one connection.
import type { AwsDsqlRetryConfig } from "@aws/aurora-dsql-drizzle";
import { and, asc, desc, eq, lt, sql, type SQL } from "drizzle-orm";
import type { Category } from "./categories.js";
import type { Db } from "./db.js";
import { comments, postCategory, posts, users } from "./schema.js";
import { pgTimestampToIso } from "./time.js";
import type { CommentView, Person, PostDetail, PostMeta, PostSummary } from "./types.js";
import {
  EXCERPT_SOURCE_CHARS,
  HttpError,
  type NewComment,
  type NewPost,
  buildPostMeta,
  excerpt,
  slugSuffix,
  slugify,
} from "./validation.js";

/** OCC retry policy for writes (kept well inside the 10 s Lambda timeout). */
const WRITE_RETRY: AwsDsqlRetryConfig = {
  maxRetries: 5,
  baseDelayMs: 20,
  maxDelayMs: 1_000,
  onRetry: (err, attempt, max) =>
    console.warn(JSON.stringify({ level: "warn", msg: "OCC conflict, retrying", attempt, max, error: String(err) })),
};

const person = {
  id: users.id,
  username: users.username,
  displayName: users.displayName,
};

const iso = pgTimestampToIso;

/** Defensive read of front matter: rows inserted by hand may not have every key. */
function meta(raw: Partial<PostMeta> | null | undefined): PostMeta {
  return {
    category: typeof raw?.category === "string" ? raw.category : "uncategorized",
    tags: Array.isArray(raw?.tags) ? raw.tags.filter((t): t is string => typeof t === "string") : [],
    readingMinutes: typeof raw?.readingMinutes === "number" ? raw.readingMinutes : 1,
    seo: { description: typeof raw?.seo?.description === "string" ? raw.seo.description : "" },
  };
}

// ---------------------------------------------------------------------------
// Reads

export async function ping(db: Db): Promise<void> {
  await db.execute(sql`select 1`);
}

export async function listUsers(db: Db, limit: number): Promise<Person[]> {
  return db.select(person).from(users).orderBy(asc(users.username)).limit(limit);
}

export interface ListPostsOptions {
  limit: number;
  category?: Category;
  /** ISO-8601 cursor: only posts created strictly before this. */
  before?: string;
}

export function listPostsWhere({ category, before }: Omit<ListPostsOptions, "limit">): SQL | undefined {
  return and(
    // metadata->>'category' = $1: the same expression posts_category_idx is built on.
    category ? eq(postCategory(posts.metadata), category) : undefined,
    before ? lt(posts.createdAt, before) : undefined,
  );
}

export function listPostsQuery(db: Db, opts: ListPostsOptions) {
  return db
    .select({
      id: posts.id,
      slug: posts.slug,
      title: posts.title,
      // Only pull the head of the body: posts are big and the list only needs an excerpt.
      bodyHead: sql<string>`left(${posts.body}, ${sql.raw(String(EXCERPT_SOURCE_CHARS))})`,
      metadata: posts.metadata,
      createdAt: posts.createdAt,
      author: person,
    })
    .from(posts)
    .innerJoin(users, eq(users.id, posts.authorId))
    .where(listPostsWhere(opts))
    .orderBy(desc(posts.createdAt))
    .limit(opts.limit);
}

export async function listPosts(db: Db, opts: ListPostsOptions): Promise<PostSummary[]> {
  const rows = await listPostsQuery(db, opts);
  return rows.map((r) => {
    const m = meta(r.metadata);
    return {
      id: r.id,
      slug: r.slug,
      title: r.title,
      excerpt: excerpt(r.bodyHead),
      category: m.category,
      tags: m.tags,
      readingMinutes: m.readingMinutes,
      createdAt: iso(r.createdAt),
      author: r.author,
    };
  });
}

async function postById(db: Pick<Db, "select">, id: string): Promise<PostDetail | undefined> {
  const [r] = await db
    .select({
      id: posts.id,
      slug: posts.slug,
      title: posts.title,
      body: posts.body,
      metadata: posts.metadata,
      createdAt: posts.createdAt,
      author: person,
    })
    .from(posts)
    .innerJoin(users, eq(users.id, posts.authorId))
    .where(eq(posts.id, id));
  if (!r) return undefined;
  return toPostDetail(r);
}

function toPostDetail(r: {
  id: string;
  slug: string;
  title: string;
  body: string;
  metadata: PostMeta;
  createdAt: string;
  author: Person;
}): PostDetail {
  const m = meta(r.metadata);
  return {
    id: r.id,
    slug: r.slug,
    title: r.title,
    excerpt: excerpt(r.body),
    category: m.category,
    tags: m.tags,
    readingMinutes: m.readingMinutes,
    createdAt: iso(r.createdAt),
    author: r.author,
    body: r.body,
    metadata: m,
  };
}

export const COMMENTS_LIMIT = 200;

/** The newest COMMENTS_LIMIT comments, returned oldest first, so new comments always show up. */
export async function listComments(db: Db, postId: string): Promise<CommentView[]> {
  const rows = await db
    .select({ id: comments.id, body: comments.body, createdAt: comments.createdAt, user: person })
    .from(comments)
    .innerJoin(users, eq(users.id, comments.userId))
    .where(eq(comments.postId, postId))
    .orderBy(desc(comments.createdAt))
    .limit(COMMENTS_LIMIT);
  return rows.reverse().map((r) => ({ id: r.id, body: r.body, createdAt: iso(r.createdAt), user: r.user }));
}

export async function getPost(
  db: Db,
  id: string,
): Promise<{ post: PostDetail; comments: CommentView[] } | undefined> {
  const post = await postById(db, id);
  if (!post) return undefined;
  return { post, comments: await listComments(db, id) };
}

// ---------------------------------------------------------------------------
// Writes. transactionWithRetry re-runs the callback on OCC conflicts (40001 /
// OC000 / OC001), so the callbacks only touch the database.

export async function createPost(db: Db, input: NewPost): Promise<PostDetail> {
  const metadata = buildPostMeta(input.body, input.category, input.tags);
  return db.transactionWithRetry(
    async (tx) => {
      const [author] = await tx.select(person).from(users).where(eq(users.id, input.authorId));
      if (!author) throw new HttpError(400, "authorId does not match a user");
      const [row] = await tx
        .insert(posts)
        .values({
          authorId: author.id,
          slug: slugify(input.title, slugSuffix()),
          title: input.title,
          body: input.body,
          metadata,
        })
        .returning();
      if (!row) throw new Error("insert returned no row");
      return toPostDetail({ ...row, author });
    },
    undefined,
    WRITE_RETRY,
  );
}

export async function addComment(
  db: Db,
  postId: string,
  input: NewComment,
): Promise<CommentView & { postId: string }> {
  return db.transactionWithRetry(
    async (tx) => {
      const [post] = await tx.select({ id: posts.id }).from(posts).where(eq(posts.id, postId));
      if (!post) throw new HttpError(404, "post not found");
      const [user] = await tx.select(person).from(users).where(eq(users.id, input.userId));
      if (!user) throw new HttpError(400, "userId does not match a user");
      const [row] = await tx
        .insert(comments)
        .values({ postId, userId: user.id, body: input.body })
        .returning({ id: comments.id, body: comments.body, createdAt: comments.createdAt });
      if (!row) throw new Error("insert returned no row");
      return { id: row.id, postId, body: row.body, createdAt: iso(row.createdAt), user };
    },
    undefined,
    WRITE_RETRY,
  );
}
