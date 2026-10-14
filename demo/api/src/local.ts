// Local dev server: `npm run dev` (tsx watch). Talks to a real cluster using
// your AWS credentials (default provider chain; AWS_PROFILE works).
//
//   CLUSTER_ENDPOINT=<id>.dsql.us-west-2.on.aws DB_USER=admin npm run dev
//
// DB_USER defaults to blog_app, which only works if your IAM principal was
// granted it (AWS IAM GRANT blog_app TO '<your role arn>'); admin is simplest.
import { serve } from "@hono/node-server";
import { createApp } from "./app.js";
import { lazyDbFromEnv } from "./db.js";

const port = Number(process.env.PORT ?? 3000);
const getDb = lazyDbFromEnv(Number(process.env.DB_POOL_MAX ?? 10));

const app = createApp({
  getDb,
  region: process.env.REGION_LABEL ?? "local",
  cors: true,
  logRequests: process.env.LOG_REQUESTS !== "0",
});

const server = serve({ fetch: app.fetch, port }, (info) => {
  const endpoint = process.env.CLUSTER_ENDPOINT ?? "(CLUSTER_ENDPOINT not set)";
  console.log(`blog api on http://localhost:${info.port} -> ${endpoint} as ${process.env.DB_USER ?? "blog_app"}`);
});

const shutdown = () => {
  server.close();
  process.exit(0);
};
process.on("SIGINT", shutdown);
process.on("SIGTERM", shutdown);
