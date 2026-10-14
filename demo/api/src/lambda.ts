// Lambda entrypoint (function URL, payload v2). One request per execution
// environment at a time, so the pool holds exactly one DSQL connection, opened
// on the first request and reused while the environment stays warm.
//
// Requests arrive through CloudFront as /api/<region>/..., so strip that
// prefix before routing. Direct calls to the function URL (signed with IAM)
// work too, with or without the prefix.
import type { LambdaContext, LambdaEvent } from "hono/aws-lambda";
import { handle } from "hono/aws-lambda";
import { createApp } from "./app.js";
import { lazyDbFromEnv } from "./db.js";

const app = createApp({
  getDb: lazyDbFromEnv(1),
  region: process.env.AWS_REGION ?? "local",
  cors: false, // same origin as the page, via CloudFront
  logRequests: process.env.LOG_REQUESTS === "1",
});

const honoHandler = handle(app);

export const stripApiPrefix = (path: string) => path.replace(/^\/api\/[a-z0-9-]+(?=\/|$)/, "") || "/";

export const handler = (event: LambdaEvent, context: LambdaContext) => {
  if ("rawPath" in event && typeof event.rawPath === "string") {
    event.rawPath = stripApiPrefix(event.rawPath);
  }
  return honoHandler(event, context);
};
