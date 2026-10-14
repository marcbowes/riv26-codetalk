// Fixed physical names. Stack names and construct ids stay fixed too, so a
// redeploy always updates in place.

export const STACKS = {
  cluster: (region: string) => `Dat401-Cluster-${region}`,
  api: (region: string) => `Dat401-Api-${region}`,
  web: "Dat401-Web",
  events: "Dat401-Events",
  dashboard: "Dat401-Dashboard",
} as const;

/** Lambda function name (per Region; the same name in each). */
export const FUNCTION_NAME = "dat401-blog-api";

/** IAM role names are global, so include the Region. setup.ts maps these to blog_app. */
export const apiRoleName = (region: string) => `dat401-blog-api-${region}`;

/** Database role the API connects as. */
export const DB_USER = "blog_app";

/** Kinesis stream the live talk's CDC (step 11) writes to. */
export const CDC_STREAM = "dat401-cdc";
/** Kinesis stream for the demo cluster's own CDC (deploy.sh, ENABLE_DEMO_CDC=1). */
export const DEMO_CDC_STREAM = "dat401-demo-cdc";
/** Service role DSQL assumes to write CDC records. */
export const CDC_ROLE = "dat401-cdc-role";

export const DASHBOARD_NAME = "DAT401-Blog";

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
