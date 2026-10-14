// Cross-column coordination inside this browser tab.
//
// When one column saves a comment, it records where and when here, then tells
// the other columns. They read immediately, so the audience sees a write in one
// Region show up on the very next read in the other (strong consistency), not
// just at the next 2-second poll.

export interface Committed {
  postId: string;
  commentId: string;
  /** Column (Region) that wrote it. */
  column: number;
  region: string;
  /** performance.now() when the POST returned 201. */
  at: number;
}

const writes = new Map<string, Committed>();
const listeners = new Set<(c: Committed) => void>();

export function publishCommit(c: Committed): void {
  writes.set(c.commentId, c);
  for (const l of listeners) l(c);
}

export function onCommit(listener: (c: Committed) => void): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

export function writtenHere(commentId: string): Committed | undefined {
  return writes.get(commentId);
}
