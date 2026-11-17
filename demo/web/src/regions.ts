// Which Regions the page shows, picked by `?region=`. Kept free of React so
// it can be tested on its own (test/regions.test.ts).
import type { RegionConfig } from "./config";

/** `?region=` value that shows every Region side by side. */
export const BOTH = "both";

/**
 * Which Regions to show, from `?region=`: a configured Region shows just that
 * one, `both` shows them all side by side, and anything else (or nothing)
 * shows the first. In production you'd send each reader to their nearest
 * Region (Route 53 latency routing, say); here the URL picks.
 */
export function pickRegions(param: string | null, regions: RegionConfig[]): { single: number; both: boolean; fromParam: boolean } {
  if (param === BOTH && regions.length > 1) return { single: 0, both: true, fromParam: false };
  const i = param === null ? -1 : regions.findIndex((r) => r.region === param);
  return i >= 0 ? { single: i, both: false, fromParam: true } : { single: 0, both: false, fromParam: false };
}

/** The page's URL with `?region=` set to `value` (or removed), other params and the hash kept. */
export function withRegionParam(href: string, value: string | undefined): string {
  const url = new URL(href);
  if (value === undefined) url.searchParams.delete("region");
  else url.searchParams.set("region", value);
  return `${url.pathname}${url.search}${url.hash}`;
}
