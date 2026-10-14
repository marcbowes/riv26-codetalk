# DAT401 demo: the polished multi-Region blog

The finished version of the blog built live in the talk, shown in minutes 35 to 45. One Aurora DSQL
database is peered across two Regions, with a Lambda API in each Region and a React page that
shows both side by side. Write a comment in us-east-1 and the next read in us-east-2 already has
it. Then `loadgen/` throws traffic at it and the CloudWatch dashboard shows Lambda and DSQL
scaling with no provisioning.

```
            CloudFront + S3 (Dat401-Web)
                    |  React UI: one column per Region
         +----------+-----------+
         v                      v
  Lambda function URL     Lambda function URL       Hono + Drizzle (@aws/aurora-dsql-drizzle),
  us-east-1               us-east-2                 Node 24 arm64, 1 DB connection per environment
         |                      |
         v                      v
  DSQL cluster  <== peered ==>  DSQL cluster        one logical database, strongly consistent
  us-east-1                     us-east-2           witness Region: us-west-2
```

| Folder | What |
|---|---|
| `api/` | Hono app (`src/app.ts`), Drizzle schema (`src/schema.ts`) and queries; `src/local.ts` (dev server), `src/lambda.ts` (handler) |
| `db/` | `setup.ts`: idempotent schema, async indexes, `blog_app` role, `AWS IAM GRANT`s, seed data |
| `infra/` | CDK app: `Dat401-Cluster-<region>` ×2, `Dat401-Api-<region>` ×2, `Dat401-Web`, `Dat401-Dashboard` |
| `web/` | Vite + React UI, reads `/config.json` at runtime |
| `loadgen/` | Rust load generator (see its own README) |

## Prerequisites

- Node.js 22.12 or newer (Lambda runs Node 24), npm, `jq`, `curl`
- AWS CLI v2 recent enough to have `aws dsql`, and credentials for an
  account with admin-level permissions. The DB setup connects as the DSQL `admin` role, so your
  principal needs `dsql:DbConnectAdmin`.
- No Docker: Lambda bundling uses the local esbuild.

## Deploy

```bash
cd demo
./deploy.sh    # about 15-25 minutes the first time (CloudFront is the slow part)
```

`deploy.sh` is idempotent and safe to re-run. It:

1. checks prerequisites, runs `npm install`
2. runs `cdk bootstrap` in both Regions
3. creates both clusters with only the witness Region (phase 1, skipped once they exist), then
   updates both with each other's ARN, concurrently (phase 2), and waits for `ACTIVE`
4. deploys both API stacks
5. runs `db/setup.ts` once against REGION_A (schema, indexes, role, IAM grants for both Lambda
   roles, seed data if the tables are empty)
6. builds the web app and deploys it with the API URLs baked into `config.json`
7. deploys the dashboard
8. writes `outputs.json` (the API URLs, cluster ids/endpoints, web and dashboard URLs, used by
   `loadgen/`) and pings each API's `/health`

Settings (environment variables): `REGION_A` (us-east-1), `REGION_B` (us-east-2),
`WITNESS_REGION` (us-west-2), `SKIP_BOOTSTRAP=1`, and `SETUP_ARGS` for extra
`db/setup.ts` flags, such as `--regrant` after recreating the API stacks, or `--skip-seed`.

Running `cdk` by hand: the app refuses to synthesize without context, because synthesizing the
cluster stacks without their peers and deploying would un-peer them. Pass
`-c clusterArns=<arnA>,<arnB>` (see `outputs.json`), plus
`-c apiUrls=us-east-1=<url>,us-east-2=<url>` for `Dat401-Web`.

## Destroy

```bash
./destroy.sh          # asks you to type "destroy"; FORCE=1 skips the prompt
```

Turns off deletion protection, then deletes the stacks in reverse order: Web and Dashboard, then
both APIs, then both clusters together (a peered cluster stays in `PENDING_DELETE` until its peer
goes too). The `CDKToolkit` bootstrap stacks stay.

## Local development

Run the API on your laptop against the deployed cluster, then the web app against it:

```bash
cd demo
npm install

# API on http://localhost:3000. Uses your AWS credentials (AWS_PROFILE works).
CLUSTER_ENDPOINT="$(jq -r '.regions[0].clusterEndpoint' outputs.json)" DB_USER=admin npm run dev:api

# In another terminal: the UI on http://localhost:5173, both columns on the local API
VITE_API_URL=http://localhost:3000 npm run dev:web

# ...or the UI against the two deployed Regions
VITE_API_URL="$(jq -r '[.regions[].apiUrl] | join(",")' outputs.json)" npm run dev:web
```

`DB_USER=admin` is simplest, since you already have `dsql:DbConnectAdmin`. To use the app's
least-privilege role instead, map your IAM role to it once:
`npm run db:setup -- --endpoint <endpoint> --skip-seed --role-arn arn:aws:iam::<acct>:role/<your role>`
(setup never revokes existing mappings) and run the API without `DB_USER`.

Checks:

```bash
npm run typecheck       # all workspaces (TypeScript 7)
npm test                # API + seed unit tests, no database needed
PG_TEST_URL=postgres://postgres@localhost:5432/postgres npm test -w @dat401/db
                        # setup steps, seed and every API route against a scratch local PostgreSQL
npm run synth -- -c clusterArns=arn:aws:dsql:us-east-1:111122223333:cluster/aaaaaaaaaaaaaaaaaaaaaaaaaa,arn:aws:dsql:us-east-2:111122223333:cluster/bbbbbbbbbbbbbbbbbbbbbbbbbb
```

## API

All JSON. Every body has `region` and `timing.dbMs` (time spent waiting on DSQL), and every
response has an `x-region` header.

| Route | |
|---|---|
| `GET /health` | `SELECT 1` |
| `GET /users?limit=50` | readers for the comment form |
| `GET /categories` | static list |
| `GET /posts?limit=20&category=&before=` | newest first; `before` is a `createdAt` from the previous page (exact to the microsecond) |
| `GET /posts/:id` | post + the latest 200 comments, oldest first |
| `POST /posts` | `{ authorId, title, body, category, tags? }` → 201 |
| `POST /posts/:id/comments` | `{ userId, body }` → 201; 404 if the post is missing; 400 on validation |

If DSQL refuses a connection (SQLSTATE 53400, more than 100 new connections per second; or 53300,
too many connections), the API answers 429 with `x-amzn-ErrorType: TooManyRequestsException`, the
same signal Lambda gives when it throttles. Clients should back off and retry.

## Scaling notes for the finale

- **Lambda concurrency.** This account's quota is 1,000 concurrent executions per Region
  (checked: `L-B99A9384` is 1,000 in us-east-1 and us-east-2), shared by every function in the
  Region. Lambda also caps synchronous invocations at 10 times that per second. Throughput per
  Region is the lower of 1,000 ÷ request duration (25,000/s at 40 ms) and 10,000/s, so with fast
  requests the cap is 10,000 requests/s. Past that, Lambda answers 429. Throttles show on the dashboard, which also
  draws the 1,000 line on the concurrency graph. For bigger numbers, request an increase of
  "Concurrent executions" (Service Quotas, AWS Lambda, `L-B99A9384`) in both Regions well before
  the talk.
- **DSQL connections.** Each Lambda environment holds one connection, reused while it's warm and
  recycled before DSQL's 60-minute limit. A cluster accepts 100 new connections per second (burst
  1,000) and 10,000 in total, so ramp the load rather than slamming it in. The 429s above are what
  you'll see if you don't.
- **Hot rows.** Comments are inserts with fresh UUIDs, so they don't conflict with each other.
  Nothing keeps counters in a single row, which keeps OCC conflicts (also on the dashboard) near
  zero.
