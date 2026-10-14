#!/usr/bin/env node
// DAT401 demo infrastructure. Use ../deploy.sh, which passes the context this
// app needs (see lib/config.ts) and deploys the stacks in order:
//
//   Dat401-Cluster-<A>, Dat401-Cluster-<B>   DSQL clusters (two-phase peering)
//   Dat401-Api-<A>, Dat401-Api-<B>           Lambda + function URL per Region
//   Dat401-Web                               S3 + CloudFront UI (REGION_A)
//   Dat401-Dashboard                         CloudWatch dashboard (REGION_A)
import * as cdk from "aws-cdk-lib";
import { ApiStack } from "../lib/api-stack";
import { ClusterStack } from "../lib/cluster-stack";
import { clusterIdFromArn, readConfig } from "../lib/config";
import { DashboardStack } from "../lib/dashboard-stack";
import { STACKS } from "../lib/names";
import { WebStack } from "../lib/web-stack";

const app = new cdk.App();
const cfg = readConfig(app);
const env = (region: string) => ({ account: cfg.account, region });

cdk.Tags.of(app).add("project", "dat401-reinvent-2026");

const allArns = cfg.clusterArns ? cfg.regions.map((r) => cfg.clusterArns?.[r] as string) : undefined;

for (const region of cfg.regions) {
  new ClusterStack(app, STACKS.cluster(region), {
    env: env(region),
    description: `DAT401 demo: Aurora DSQL cluster in ${region} (multi-Region, witness ${cfg.witnessRegion})`,
    witnessRegion: cfg.witnessRegion,
    ...(allArns ? { clusterArns: allArns } : {}),
  });
}

if (cfg.clusterArns) {
  const arns = cfg.clusterArns;

  for (const region of cfg.regions) {
    new ApiStack(app, STACKS.api(region), {
      env: env(region),
      description: `DAT401 demo: blog API in ${region}`,
      clusterArn: arns[region] as string,
    });
  }

  new DashboardStack(app, STACKS.dashboard, {
    env: env(cfg.regionA),
    description: "DAT401 demo: cross-Region CloudWatch dashboard",
    witnessRegion: cfg.witnessRegion,
    clusters: cfg.regions.map((region) => ({ region, clusterId: clusterIdFromArn(arns[region] as string) })),
  });

  // Needs the API URLs (another Region's outputs), so deploy.sh passes them in.
  if (cfg.apiUrls) {
    const urls = cfg.apiUrls;
    new WebStack(app, STACKS.web, {
      env: env(cfg.regionA),
      description: "DAT401 demo: static React UI on S3 + CloudFront",
      witnessRegion: cfg.witnessRegion,
      regions: cfg.regions.map((region) => ({ region, apiUrl: urls[region] as string })),
    });
  }
}
