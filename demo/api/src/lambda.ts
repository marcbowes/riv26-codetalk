// Lambda entrypoint (function URL, payload v2). One request per execution
// environment at a time, so the pool holds exactly one DSQL connection, opened
// on the first request and reused while the environment stays warm.
import { handle } from "hono/aws-lambda";
import { createApp } from "./app.js";
import { lazyDbFromEnv } from "./db.js";

const app = createApp({
  getDb: lazyDbFromEnv(1),
  region: process.env.AWS_REGION ?? "local",
  cors: false, // the function URL's CORS config handles it
  logRequests: process.env.LOG_REQUESTS === "1",
});

export const handler = handle(app);
