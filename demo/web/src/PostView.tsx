import type { CommentView, Person, PostDetail } from "@dat401/api/types";
import { useCallback, useEffect, useRef, useState } from "react";
import { ApiError, type Client } from "./api";
import type { View } from "./App";
import { onCommit, publishCommit, writtenHere } from "./bus";
import { ago, categoryLabel, ms } from "./format";

const POLL_MS = 2_000;
const HIGHLIGHT_MS = 8_000;

type Arrival =
  /** Written in another column (Region) of this page; lagMs = commit there → visible here. */
  | { kind: "remote"; column: number; region: string; lagMs: number }
  /** Written in this column. */
  | { kind: "local" }
  /** Written somewhere else entirely (another browser, the load generator). */
  | { kind: "new" };

export function PostView({
  client,
  id,
  column,
  region,
  users,
  navigate,
}: {
  client: Client;
  id: string;
  column: number;
  region: string;
  users: Person[] | null;
  navigate: (v: View) => void;
}) {
  const [post, setPost] = useState<PostDetail | null>(null);
  const [comments, setComments] = useState<CommentView[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [arrivals, setArrivals] = useState<Record<string, Arrival>>({});

  // Comments already on screen: anything not in here is an arrival.
  const seen = useRef<Set<string> | null>(null);
  // Polls and nudges can overlap; never apply an older response over a newer one.
  const issued = useRef(0);
  const applied = useRef(0);

  const markArrived = useCallback((next: Record<string, Arrival>) => {
    setArrivals((a) => ({ ...a, ...next }));
    setTimeout(
      () =>
        setArrivals((a) => {
          const b = { ...a };
          for (const k of Object.keys(next)) delete b[k];
          return b;
        }),
      HIGHLIGHT_MS,
    );
  }, []);

  const refresh = useCallback(async () => {
    const seq = ++issued.current;
    try {
      const r = await client.post(id);
      if (seq < applied.current) return;
      applied.current = seq;
      const now = performance.now();
      setPost(r.post);
      setError(null);
      if (seen.current === null) {
        seen.current = new Set(r.comments.map((c) => c.id));
      } else {
        const next: Record<string, Arrival> = {};
        for (const c of r.comments) {
          if (seen.current.has(c.id)) continue;
          seen.current.add(c.id);
          const w = writtenHere(c.id);
          next[c.id] =
            w && w.column !== column
              ? { kind: "remote", column: w.column, region: w.region, lagMs: now - w.at }
              : w
                ? { kind: "local" }
                : { kind: "new" };
        }
        if (Object.keys(next).length > 0) markArrived(next);
      }
      setComments(r.comments);
    } catch (err) {
      if (seq < applied.current) return;
      setError(
        err instanceof ApiError && err.status === 404
          ? "This post doesn't exist (any more)."
          : `Can't load this post from ${region}: ${err instanceof Error ? err.message : String(err)}. Retrying.`,
      );
    }
  }, [client, id, column, region, markArrived]);

  // Poll every 2 s while the tab is visible.
  useEffect(() => {
    let stopped = false;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const loop = async () => {
      if (stopped) return;
      if (!document.hidden) await refresh();
      if (!stopped) timer = setTimeout(loop, POLL_MS);
    };
    void loop();
    return () => {
      stopped = true;
      clearTimeout(timer);
    };
  }, [refresh]);

  // Another column just saved a comment on this post: read it right away.
  useEffect(
    () =>
      onCommit((c) => {
        if (c.postId === id && c.column !== column) void refresh();
      }),
    [id, column, refresh],
  );

  const onPosted = useCallback(
    (c: CommentView) => {
      seen.current?.add(c.id);
      setComments((cs) => (cs.some((x) => x.id === c.id) ? cs : [...cs, c]));
      markArrived({ [c.id]: { kind: "local" } });
    },
    [markArrived],
  );

  if (!post) {
    return (
      <div className="post-view">
        <BackLink navigate={navigate} />
        {error ? (
          <p className="notice" role="alert">
            {error}
          </p>
        ) : (
          <p className="empty">Loading…</p>
        )}
      </div>
    );
  }

  const blocks = post.body.split(/\n{2,}/).filter((b) => b.trim());
  const newestFirst = [...comments].reverse();

  return (
    <article className="post-view">
      <BackLink navigate={navigate} />
      <h2 className="post-title">{post.title}</h2>
      <p className="meta">
        <span className="author">{post.author.displayName}</span>
        <span>{ago(post.createdAt)}</span>
        <span>{post.readingMinutes} min read</span>
        <button type="button" className="category" onClick={() => navigate({ kind: "list", category: post.category })}>
          {categoryLabel(post.category)}
        </button>
      </p>
      {post.tags.length > 0 ? (
        <ul className="tags" aria-label="Tags">
          {post.tags.map((t) => (
            <li key={t}>#{t}</li>
          ))}
        </ul>
      ) : null}

      <div className="post-body">
        {blocks.map((b, i) => (b.startsWith("## ") ? <h3 key={i}>{b.slice(3)}</h3> : <p key={i}>{b}</p>))}
      </div>

      <section className="comments" aria-label="Comments">
        <h3 className="comments-title">
          {comments.length === 0 ? "No comments yet" : `${comments.length} comment${comments.length === 1 ? "" : "s"}`}
        </h3>
        <CommentForm client={client} postId={post.id} column={column} region={region} users={users} onPosted={onPosted} />
        {error ? <p className="notice small">{error}</p> : null}
        {comments.length === 0 ? <p className="empty">Write the first one. It's saved once and readable in both Regions.</p> : null}
        <ol className="comment-list">
          {newestFirst.map((c) => (
            <Comment key={c.id} comment={c} arrival={arrivals[c.id]} column={column} />
          ))}
        </ol>
        {comments.length >= 200 ? <p className="empty small">Showing the latest 200 comments.</p> : null}
      </section>
    </article>
  );
}

function BackLink({ navigate }: { navigate: (v: View) => void }) {
  return (
    <button type="button" className="back" onClick={() => navigate({ kind: "list" })}>
      All posts
    </button>
  );
}

function Comment({ comment, arrival, column }: { comment: CommentView; arrival: Arrival | undefined; column: number }) {
  const origin = arrival?.kind === "remote" ? arrival.column : column;
  return (
    <li
      className={`comment${arrival ? ` arrived ${arrival.kind}` : ""}`}
      style={arrival ? ({ "--origin": `var(--region-${origin % 2})` } as React.CSSProperties) : undefined}
    >
      <p className="comment-head">
        <span className="author">{comment.user.displayName}</span>
        <span className="handle">@{comment.user.username}</span>
        <span className="when">{ago(comment.createdAt)}</span>
      </p>
      <p className="comment-body">{comment.body}</p>
      {arrival?.kind === "remote" ? (
        <p className="arrival-note">
          Posted in {arrival.region}, read here {ms(arrival.lagMs)} after it was saved.
        </p>
      ) : arrival?.kind === "new" ? (
        <p className="arrival-note">New</p>
      ) : null}
    </li>
  );
}

function CommentForm({
  client,
  postId,
  column,
  region,
  users,
  onPosted,
}: {
  client: Client;
  postId: string;
  column: number;
  region: string;
  users: Person[] | null;
  onPosted: (c: CommentView) => void;
}) {
  const [userId, setUserId] = useState<string>("");
  const [text, setText] = useState("");
  const [posting, setPosting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [lastMs, setLastMs] = useState<number | null>(null);

  // Each column starts as a different person (picked from its own slice of the
  // list), so the two sides read as two readers.
  useEffect(() => {
    if (!userId && users && users.length > 0) {
      const slice = Math.max(1, Math.floor(users.length / 2));
      const pick = users[((column % 2) * slice + Math.floor(Math.random() * slice)) % users.length];
      if (pick) setUserId(pick.id);
    }
  }, [users, userId, column]);

  const submit = async () => {
    const body = text.trim();
    if (!body || !userId || posting) return;
    setPosting(true);
    setError(null);
    const started = performance.now();
    try {
      const r = await client.addComment(postId, userId, body);
      const at = performance.now();
      setLastMs(at - started);
      setText("");
      onPosted(r.comment);
      publishCommit({ postId, commentId: r.comment.id, column, region, at });
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setPosting(false);
    }
  };

  const fieldId = `comment-${column}`;
  return (
    <form
      className="comment-form"
      onSubmit={(e) => {
        e.preventDefault();
        void submit();
      }}
    >
      <div className="as">
        <label htmlFor={`${fieldId}-user`}>Comment as</label>
        <select
          id={`${fieldId}-user`}
          value={userId}
          onChange={(e) => setUserId(e.target.value)}
          disabled={!users}
        >
          {!users ? <option value="">Loading readers…</option> : null}
          {users?.map((u) => (
            <option key={u.id} value={u.id}>
              {u.displayName} (@{u.username})
            </option>
          ))}
        </select>
      </div>
      <label htmlFor={fieldId} className="visually-hidden">
        Comment
      </label>
      <textarea
        id={fieldId}
        rows={1}
        maxLength={2000}
        value={text}
        placeholder={`Write a comment from ${region}`}
        onChange={(e) => setText(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) {
            e.preventDefault();
            void submit();
          }
        }}
      />
      <button type="submit" className="send" disabled={posting || !text.trim() || !userId}>
        {posting ? "Posting…" : `Post in ${region}`}
      </button>
      {error ? (
        <p className="form-status form-error" role="alert">
          Not posted: {error}
        </p>
      ) : lastMs !== null ? (
        <p className="form-status">Saved in {region} in {ms(lastMs)}, round trip.</p>
      ) : null}
    </form>
  );
}
