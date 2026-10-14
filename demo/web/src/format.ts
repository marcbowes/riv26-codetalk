const rtf = new Intl.RelativeTimeFormat("en", { numeric: "auto" });

const STEPS: [Intl.RelativeTimeFormatUnit, number][] = [
  ["year", 365 * 86_400_000],
  ["month", 30 * 86_400_000],
  ["week", 7 * 86_400_000],
  ["day", 86_400_000],
  ["hour", 3_600_000],
  ["minute", 60_000],
];

export function ago(iso: string, now = Date.now()): string {
  const diff = Date.parse(iso) - now;
  for (const [unit, ms] of STEPS) {
    if (Math.abs(diff) >= ms) return rtf.format(Math.round(diff / ms), unit);
  }
  return "just now";
}

export function categoryLabel(slug: string): string {
  const s = slug.replace(/-/g, " ");
  return s.charAt(0).toUpperCase() + s.slice(1);
}

export function ms(n: number | undefined): string {
  if (n === undefined || Number.isNaN(n)) return "–";
  if (n >= 1000) return `${(n / 1000).toFixed(n >= 10_000 ? 0 : 1)} s`;
  return `${n >= 100 ? Math.round(n) : n.toFixed(n >= 10 ? 0 : 1)} ms`;
}

export function median(xs: number[]): number | undefined {
  if (xs.length === 0) return undefined;
  const s = [...xs].sort((a, b) => a - b);
  const mid = Math.floor(s.length / 2);
  return s.length % 2 ? s[mid] : ((s[mid - 1] as number) + (s[mid] as number)) / 2;
}

/** What a request did, in words (for the column header). */
export function describeRequest(method: string, path: string): string {
  const p = path.split("?")[0] ?? path;
  if (method === "POST" && /^\/posts\/[^/]+\/comments$/.test(p)) return "saved a comment";
  if (method === "POST" && p === "/posts") return "saved a post";
  if (/^\/posts\/[^/]+$/.test(p)) return "read a post and its comments";
  if (p === "/posts") return "listed posts";
  if (p === "/users") return "loaded readers";
  if (p === "/health") return "health check";
  return `${method} ${p}`;
}
