// Tiny typed client for one Region's API. Every call reports its client-side
// round trip and the server's dbMs to `onSample`, which feeds the column header.
import type {
  CategoriesResponse,
  CreateCommentResponse,
  Envelope,
  ErrorResponse,
  PostResponse,
  PostsResponse,
  UsersResponse,
} from "@dat401/api/types";

export interface Sample {
  path: string;
  method: string;
  status: number;
  /** Client-measured round trip, including JSON parsing. */
  ms: number;
  /** Time the API spent waiting on Aurora DSQL. */
  dbMs: number | undefined;
  /** Region the API says it ran in. */
  region: string | undefined;
  at: number;
}

export class ApiError extends Error {
  constructor(
    readonly status: number,
    message: string,
  ) {
    super(message);
  }
}

export type Client = ReturnType<typeof createClient>;

/**
 * Hex SHA-256 of a request body. CloudFront's origin access control signs
 * requests to the Lambda function URLs but can't hash bodies itself, so POSTs
 * must carry x-amz-content-sha256.
 */
async function sha256Hex(text: string): Promise<string> {
  const digest = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(text));
  return Array.from(new Uint8Array(digest), (b) => b.toString(16).padStart(2, "0")).join("");
}

export function createClient(baseUrl: string, onSample: (s: Sample) => void) {
  const base = baseUrl.replace(/\/+$/, "");

  async function call<T extends Envelope>(
    method: "GET" | "POST",
    path: string,
    body?: unknown,
    { sample = true }: { sample?: boolean } = {},
  ): Promise<T> {
    const started = performance.now();
    const ctl = new AbortController();
    const timer = setTimeout(() => ctl.abort(), 9_000);
    let status = 0;
    let parsed: (T & Partial<ErrorResponse>) | undefined;
    try {
      let init: RequestInit = { method, signal: ctl.signal };
      if (body !== undefined) {
        const json = JSON.stringify(body);
        init = {
          ...init,
          headers: { "content-type": "application/json", "x-amz-content-sha256": await sha256Hex(json) },
          body: json,
        };
      }
      const res = await fetch(`${base}${path}`, init);
      status = res.status;
      parsed = (await res.json().catch(() => undefined)) as typeof parsed;
      if (!res.ok) throw new ApiError(status, parsed?.error ?? `HTTP ${status}`);
      if (!parsed) throw new ApiError(status, "response was not JSON");
      return parsed;
    } catch (err) {
      if (err instanceof ApiError) throw err;
      const aborted = err instanceof DOMException && err.name === "AbortError";
      throw new ApiError(0, aborted ? "request timed out" : `could not reach the API (${String(err)})`);
    } finally {
      clearTimeout(timer);
      if (sample) onSample({
        path,
        method,
        status,
        ms: performance.now() - started,
        dbMs: parsed?.timing?.dbMs,
        region: parsed?.region,
        at: Date.now(),
      });
    }
  }

  return {
    // Static list, no database work: keep it out of the latency readout.
    categories: () => call<CategoriesResponse>("GET", "/categories", undefined, { sample: false }),
    users: (limit = 50) => call<UsersResponse>("GET", `/users?limit=${limit}`),
    posts: (opts: { category?: string; before?: string; limit?: number } = {}) => {
      const q = new URLSearchParams({ limit: String(opts.limit ?? 20) });
      if (opts.category) q.set("category", opts.category);
      if (opts.before) q.set("before", opts.before);
      return call<PostsResponse>("GET", `/posts?${q}`);
    },
    post: (id: string) => call<PostResponse>("GET", `/posts/${encodeURIComponent(id)}`),
    addComment: (postId: string, userId: string, body: string) =>
      call<CreateCommentResponse>("POST", `/posts/${encodeURIComponent(postId)}/comments`, { userId, body }),
  };
}
