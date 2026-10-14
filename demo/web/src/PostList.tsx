import type { PostSummary } from "@dat401/api/types";
import { useCallback, useEffect, useState } from "react";
import { ApiError, type Client } from "./api";
import type { View } from "./App";
import { ago, categoryLabel } from "./format";

const PAGE = 20;

export function PostList({
  client,
  category,
  categories,
  navigate,
}: {
  client: Client;
  category: string | undefined;
  categories: string[];
  navigate: (v: View) => void;
}) {
  const [posts, setPosts] = useState<PostSummary[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [more, setMore] = useState(true);

  const load = useCallback(
    async (before?: string) => {
      setLoading(true);
      try {
        const r = await client.posts({ ...(category ? { category } : {}), ...(before ? { before } : {}), limit: PAGE });
        setPosts((ps) => (before ? [...ps, ...r.posts] : r.posts));
        setMore(r.posts.length === PAGE);
        setError(null);
      } catch (err) {
        setError(err instanceof ApiError ? err.message : String(err));
      } finally {
        setLoading(false);
      }
    },
    [client, category],
  );

  useEffect(() => {
    void load();
  }, [load]);

  return (
    <div className="post-list">
      <nav className="chips" aria-label="Categories">
        <button type="button" aria-pressed={!category} onClick={() => navigate({ kind: "list" })}>
          All
        </button>
        {categories.map((c) => (
          <button key={c} type="button" aria-pressed={category === c} onClick={() => navigate({ kind: "list", category: c })}>
            {categoryLabel(c)}
          </button>
        ))}
      </nav>

      {error ? (
        <div className="notice" role="alert">
          <p>Couldn't load posts: {error}</p>
          <button type="button" onClick={() => void load()}>
            Try again
          </button>
        </div>
      ) : null}

      {!error && !loading && posts.length === 0 ? (
        <p className="empty">No posts {category ? `in ${categoryLabel(category)} ` : ""}yet.</p>
      ) : null}

      <ol className="posts">
        {posts.map((p) => (
          <li key={p.id} className="post-summary">
            <h3>
              <a
                href={`#${p.slug}`}
                onClick={(e) => {
                  e.preventDefault();
                  navigate({ kind: "post", id: p.id });
                }}
              >
                {p.title}
              </a>
            </h3>
            <p className="excerpt">{p.excerpt}</p>
            <p className="meta">
              <span className="author">{p.author.displayName}</span>
              <span>{ago(p.createdAt)}</span>
              <span>{p.readingMinutes} min read</span>
              <span className="category">{categoryLabel(p.category)}</span>
            </p>
          </li>
        ))}
      </ol>

      {posts.length > 0 && more ? (
        <button type="button" className="more" disabled={loading} onClick={() => void load(posts.at(-1)?.createdAt)}>
          {loading ? "Loading…" : "Older posts"}
        </button>
      ) : null}
      {loading && posts.length === 0 ? <p className="empty">Loading posts…</p> : null}
    </div>
  );
}
