// Context → config. deploy.sh passes everything via `-c key=value`:
//
//   regionA, regionB, witnessRegion   (defaults us-east-1, us-east-2, us-west-2)
//   clusterPhase=create               first deploy only: cluster stacks, no peers
//   clusterArns=<arnA>,<arnB>         every other deploy (peering + API wiring)
//   apiUrls=<region>=<url>,<region>=<url>   adds the Web stack
//
// Without clusterPhase=create the cluster ARNs are mandatory: synthesizing a
// cluster stack without its peers and deploying it would un-peer the clusters.
import type { App } from "aws-cdk-lib";

export interface DemoConfig {
  account: string | undefined;
  regionA: string;
  regionB: string;
  regions: [string, string];
  witnessRegion: string;
  /** First deploy: only the cluster stacks, created without peers. */
  createPhase: boolean;
  /** region → cluster ARN (absent only in the create phase). */
  clusterArns: Record<string, string> | undefined;
  /** region → function URL (only when deploying the Web stack). */
  apiUrls: Record<string, string> | undefined;
}

const REGION_RE = /^[a-z]{2}(-[a-z]+)+-\d$/;
const CLUSTER_ARN_RE = /^arn:aws[a-z-]*:dsql:([a-z0-9-]+):(\d{12}):cluster\/([a-z0-9]+)$/;

function ctx(app: App, key: string): string | undefined {
  const v: unknown = app.node.tryGetContext(key);
  if (v === undefined || v === null || v === "") return undefined;
  return String(v).trim();
}

export function clusterIdFromArn(arn: string): string {
  const m = CLUSTER_ARN_RE.exec(arn);
  if (!m?.[3]) throw new Error(`not a DSQL cluster ARN: ${arn}`);
  return m[3];
}

export function readConfig(app: App): DemoConfig {
  const regionA = ctx(app, "regionA") ?? "us-east-1";
  const regionB = ctx(app, "regionB") ?? "us-east-2";
  const witnessRegion = ctx(app, "witnessRegion") ?? "us-west-2";
  for (const r of [regionA, regionB, witnessRegion]) {
    if (!REGION_RE.test(r)) throw new Error(`not a Region name: ${r}`);
  }
  if (new Set([regionA, regionB, witnessRegion]).size !== 3) {
    throw new Error(`regionA, regionB and witnessRegion must all differ (got ${regionA}, ${regionB}, ${witnessRegion})`);
  }
  const regions: [string, string] = [regionA, regionB];

  const createPhase = ctx(app, "clusterPhase") === "create";
  const rawArns = ctx(app, "clusterArns");
  let clusterArns: Record<string, string> | undefined;
  if (rawArns) {
    clusterArns = {};
    for (const arn of rawArns.split(",").map((s) => s.trim()).filter(Boolean)) {
      const m = CLUSTER_ARN_RE.exec(arn);
      if (!m?.[1]) throw new Error(`clusterArns: not a DSQL cluster ARN: ${arn}`);
      clusterArns[m[1]] = arn;
    }
    for (const r of regions) {
      if (!clusterArns[r]) throw new Error(`clusterArns: missing the ARN for ${r} (got ${rawArns})`);
    }
  }
  if (createPhase && clusterArns) throw new Error("pass either clusterPhase=create or clusterArns, not both");
  if (!createPhase && !clusterArns) {
    throw new Error(
      "missing context. Use deploy.sh, or pass -c clusterArns=<regionA cluster ARN>,<regionB cluster ARN> " +
        "(or -c clusterPhase=create for the very first cluster deploy). Synthesizing the cluster stacks " +
        "without their peers would un-peer existing clusters.",
    );
  }

  const rawUrls = ctx(app, "apiUrls");
  let apiUrls: Record<string, string> | undefined;
  if (rawUrls) {
    apiUrls = {};
    for (const pair of rawUrls.split(",").map((s) => s.trim()).filter(Boolean)) {
      const i = pair.indexOf("=");
      const region = pair.slice(0, i);
      const url = pair.slice(i + 1);
      if (i < 1 || !regions.includes(region) || !/^https:\/\/\S+$/.test(url)) {
        throw new Error(`apiUrls: expected <region>=https://..., got ${pair}`);
      }
      apiUrls[region] = url.endsWith("/") ? url : `${url}/`;
    }
    for (const r of regions) if (!apiUrls[r]) throw new Error(`apiUrls: missing ${r}`);
  }

  return {
    account: process.env.CDK_DEFAULT_ACCOUNT,
    regionA,
    regionB,
    regions,
    witnessRegion,
    createPhase,
    clusterArns,
    apiUrls,
  };
}
