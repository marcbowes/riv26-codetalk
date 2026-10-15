#!/usr/bin/env node
// DAT401 demo infrastructure. Use ../deploy.sh, which passes the context this
// app needs (see lib/config.ts) and deploys the stacks in order:
//
//   Dat401-Cluster-<A>, Dat401-Cluster-<B>   DSQL clusters (two-phase peering)
//   Dat401-Api-<A>, Dat401-Api-<B>           Lambda + function URL per Region
//   Dat401-Web                               S3 + CloudFront UI (REGION_A)
//   Dat401-Dashboard                         CloudWatch dashboard (REGION_A)
//   Dat401-Loadgen                           EC2 load generator host (REGION_A)
import * as cdk from "aws-cdk-lib";
import { ApiStack } from "../lib/api-stack";
import { ClusterStack } from "../lib/cluster-stack";
import { clusterIdFromArn, readConfig } from "../lib/config";
import { DashboardStack } from "../lib/dashboard-stack";
import { LoadgenStack } from "../lib/loadgen-stack";
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
      ...(cfg.distributionArn ? { distributionArn: cfg.distributionArn } : {}),
    });
  }

  new DashboardStack(app, STACKS.dashboard, {
    env: env(cfg.regionA),
    description: "DAT401 demo: cross-Region CloudWatch dashboard",
    witnessRegion: cfg.witnessRegion,
    clusters: cfg.regions.map((region) => ({ region, clusterId: clusterIdFromArn(arns[region] as string) })),
  });

  new LoadgenStack(app, STACKS.loadgen, {
    env: env(cfg.regionA),
    description: "DAT401 demo: load generator host (EC2, reached via SSM)",
    repoUrl: cfg.repoUrl,
    instanceType: cfg.loadgenInstanceType,
    ...(cfg.loadgenAz ? { availabilityZone: cfg.loadgenAz } : {}),
  });

  // Needs both function URLs (one is another Region's output), so deploy.sh passes them in.
  if (cfg.functionUrls) {
    const urls = cfg.functionUrls;
    new WebStack(app, STACKS.web, {
      env: env(cfg.regionA),
      description: "DAT401 demo: React UI on S3 + CloudFront, and /api/<region>/* to each Region's API",
      witnessRegion: cfg.witnessRegion,
      regions: cfg.regions.map((region) => ({ region, functionUrl: urls[region] as string })),
    });
  }
}
