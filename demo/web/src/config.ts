// Runtime config. Deployed: /config.json, written by the Web stack with each
// Region's function URL. Local dev: VITE_API_URL (one URL, or a comma-separated
// list); one URL is shown in both columns so you can see the layout and the
// cross-column updates against a single local API.

export interface RegionConfig {
  region: string;
  apiUrl: string;
  name?: string;
}

export interface AppConfig {
  regions: RegionConfig[];
  witnessRegion?: string;
  source: "config.json" | "VITE_API_URL";
}

export const REGION_NAMES: Record<string, string> = {
  "us-east-1": "N. Virginia",
  "us-east-2": "Ohio",
  "us-west-2": "Oregon",
  "eu-west-1": "Ireland",
  "eu-west-2": "London",
  "eu-west-3": "Paris",
  "eu-central-1": "Frankfurt",
  "ap-northeast-1": "Tokyo",
  "ap-northeast-2": "Seoul",
  "ap-northeast-3": "Osaka",
};

function isRegionConfig(x: unknown): x is RegionConfig {
  const r = x as RegionConfig;
  return typeof r?.region === "string" && typeof r?.apiUrl === "string";
}

export async function loadConfig(): Promise<AppConfig> {
  try {
    const res = await fetch("/config.json", { cache: "no-store" });
    if (res.ok && res.headers.get("content-type")?.includes("json")) {
      const body = (await res.json()) as { regions?: unknown; witnessRegion?: unknown };
      if (Array.isArray(body.regions) && body.regions.length > 0 && body.regions.every(isRegionConfig)) {
        return {
          regions: body.regions,
          ...(typeof body.witnessRegion === "string" ? { witnessRegion: body.witnessRegion } : {}),
          source: "config.json",
        };
      }
    }
  } catch {
    // fall through to the dev fallback
  }
  const urls = (import.meta.env.VITE_API_URL ?? "")
    .split(",")
    .map((s: string) => s.trim())
    .filter(Boolean);
  if (urls.length === 0) {
    throw new Error(
      "No API configured. Deployed builds read /config.json; for local dev set VITE_API_URL, " +
        "e.g. VITE_API_URL=http://localhost:3000 npm run dev -w @dat401/web",
    );
  }
  const list = urls.length === 1 ? [urls[0] as string, urls[0] as string] : urls;
  // The column header shows the Region the API reports once the first response arrives.
  return { regions: list.map((apiUrl: string) => ({ region: "local", apiUrl })), source: "VITE_API_URL" };
}
