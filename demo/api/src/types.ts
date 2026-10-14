// Wire types for the HTTP API (see the spec's "HTTP API" section). Dependency-
// free so the web app can `import type` them from "@dat401/api/types".

/** Front matter stored in posts.metadata (jsonb). */
export interface PostMeta {
  category: string;
  tags: string[];
  readingMinutes: number;
  seo: { description: string };
}

export interface Person {
  id: string;
  username: string;
  displayName: string;
}

export interface PostSummary {
  id: string;
  slug: string;
  title: string;
  excerpt: string;
  category: string;
  tags: string[];
  readingMinutes: number;
  /** ISO-8601 */
  createdAt: string;
  author: Person;
}

export interface PostDetail extends PostSummary {
  body: string;
  metadata: PostMeta;
}

export interface CommentView {
  id: string;
  body: string;
  /** ISO-8601 */
  createdAt: string;
  user: Person;
}

/** Present on every response body. */
export interface Envelope {
  region: string;
  timing: { dbMs: number };
}

export type HealthResponse = Envelope & { ok: boolean; error?: string };
export type UsersResponse = Envelope & { users: Person[] };
export type CategoriesResponse = Envelope & { categories: string[] };
export type PostsResponse = Envelope & { posts: PostSummary[] };
export type PostResponse = Envelope & { post: PostDetail; comments: CommentView[] };
export type CreatePostResponse = Envelope & { post: PostDetail };
export type CreateCommentResponse = Envelope & { comment: CommentView & { postId: string } };
export type ErrorResponse = Envelope & { error: string; details?: string[] };

export interface CreatePostRequest {
  authorId: string;
  title: string;
  body: string;
  category: string;
  tags?: string[];
}

export interface CreateCommentRequest {
  userId: string;
  body: string;
}
