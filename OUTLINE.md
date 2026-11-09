# DAT401 (re:Invent 2026): draft outline

Same title and abstract as 2025 ("Amazon Aurora DSQL: A developer's perspective"),
different talk. 2025 hand-built a transfer API from raw `pg`. 2026 builds a blog
live, using the web console where the visuals matter and psql where the exact
results matter. Terminal commands are typed by demo-it in Emacs (`stage/`),
generated from TALK.md. No AI runs on stage: step 4 shows the prompt that built
the API, then the real code. It ends with a polished version of the same app
running serverless, multi-Region and at scale.

## Ground rules for slow steps

Anything that takes a while to finish (an index build, a data load) must:

1. have a high success rate (rehearsed many times, with a fallback snapshot),
2. not take too long, and
3. leave room to explain or teach something while it runs.

## Domain: a blog

`users`, `posts`, `comments`. Foreign keys are natural
(`comments.post_id → posts.id`, `comments.user_id → users.id`), traffic is ~90/10
read/write, and multi-Region gives fast page loads everywhere. `posts.metadata`
is JSONB front matter (`{ category, tags, seo, readingMinutes }`) for the
expression-index beat. Post bodies are big; mention that DSQL compresses large
values automatically, with no demo.

## Run of show

### 0:00–5:00 Intro

DSQL combines the serverless scale of DynamoDB with the relational model of
PostgreSQL. Then the agenda.

### 5:00–35:00 Build it

| # | Step | Tool | Notes |
|---|---|---|---|
| 1 | Create a cluster in the console (for show: step 8 deletes it), then the real one by typing `aws dsql create-cluster` (demo-it reads the endpoint from its output). Connect with psql and type a small schema by hand (`sql/00-by-hand.sql`): posts with a UUID key, and why not a sequence (said, not shown: it needs a cache; `CACHE 1` dense but a round trip per id, `CACHE 65536` fast but gappy). Comments: `PRIMARY KEY (post_id, comment_id)`, random vs grouped keys (spread writes vs read locality; a per-post hotspot is fine). | console, demo-it, psql | Cluster creation takes seconds (CLI: ACTIVE in 4 s). The console cluster exists for step 9. Clusters show up as random IDs, so mixing them up later is believable. |
| 2 | Insert two posts; `select *` to show them; `\gset` the first post's id; insert a comment with `:'post_id'`. By hand. | psql | Merge with 3 if short on time. |
| 3 | `\timing on`; recent posts and comments for a post: a couple of ms each. "Hold up: they're fast because there's nothing here." | psql | No drop here any more: that's the start of step 4. |
| 4 | "The schema I typed is missing what a real app needs": drop it, `\i sql/01-schema.sql`, `\d+ posts` (metadata is JSON), `\d+ users` (comments have users). Then the loader (it adds the first rows too), the home page again (~200 ms: kind of slow), and EXPLAIN. `EXPLAIN ANALYZE VERBOSE`: home page = full scan + sort (~208 ms, ~121 DPU); comments for a post = PK range scan already (0.5 ms, 0.02 DPU; keyed by comment id alone it was 1 s / 191 DPU). | shell, psql | Pre-built: in the first dry run, building it with AI took 8 minutes. |
| 5 | Add the home-page index, covering from the start: `posts (created_at) INCLUDE (title)` (the table is its PK B-tree, so a non-covering index needs a lookup per row): 208 ms → 0.7 ms. The non-covering index (and the planner ignoring it) is cut (2026-11-09). No comments index needed. | psql | `CREATE INDEX ASYNC` → job in `sys.jobs` → `wait_for_job` → EXPLAIN again. |
| 6 | FK race: two psql sessions side by side. A deletes a post while B comments on it, and both commit, leaving an orphan. Add the FK `NOT VALID`; validation fails on the orphan. Clean up, validate (a job), and re-run the race. | psql ×2 | Good teaching moment: why both commit without the FK (no write-write conflict) and what the FK changes under OCC. |
| 7 | Query posts by category from `metadata`, then add the expression index on `metadata->>'category'`. Show `->` vs `->>` missing the index. | psql | See the backfill note under Risks. |
| 8 | Scale to zero: in the console there are two clusters; the console one has never been used and costs nothing (CloudWatch: zero DPU). Delete it: turn off deletion protection first (leave it on when creating it), then delete. | console | Delete B, not A. Point-in-time restore was cut (2026-11-05). |
| 9 | Create the Kinesis stream and the CDC stream with the CLI (`aws dsql create-stream`, then `wait stream-active`; the `dat401-cdc` role is set up beforehand). A pre-built CLI tool tails the events. Add a comment from psql and watch them arrive. | console, CLI | Kinesis stream and CDC role set up by hand (not part of `demo/`). CDC goes on cluster A. |
| 10 | Multi-Region, explained verbally while walking through setup in the console. | console | Just show the flow; the real one is already built. |

### 35:00–40:00 The polished app

The same blog, polished and already deployed (`demo/`): a React page served
from S3 + CloudFront, calling a Lambda API in each of two Regions, backed by a
multi-Region cluster. Write in one Region and read it in the other right away.

### 40:00–45:00 Wow: load

Generate load (`demo/loadgen`). Lambda and DSQL scale out automatically with
no intervention. Live TPS.

The API (old step 4) moved to the start of the polished app, at 35:00, just before it's used and load tested (2026-11-09): the prompt typed into Claude Code (not submitted), then `db.ts` and `queries.ts`.

## Risks and things to settle

- **Timing.** Eleven steps in 30 minutes. Easiest cuts if it runs long: merge 2
  and 3, and keep 11 to about a minute. There's no slack for Q&A.
- **CDC (step 10):** `CreateStream` requires the cluster to be `ACTIVE`; the blog's
  cluster is busy by then, so that's fine. The Kinesis stream needs `MaxRecordSizeInKiB` 10240.
- **Per-transaction limits (3,000 rows, 10 MiB):**
  - Sample data (step 5) has to load in batches.
  - A metadata backfill in step 8 (`UPDATE` across all posts) would hit the limit.
  - Simplest fix: put `metadata` in the step-1 schema and have step 5's
    generator fill it in. Step 8 then says "we've been storing front matter
    all along" and goes straight to querying.
- **FK race specifics (step 7).** Settled by the FK docs: with the FK in place,
  the deleter commits and the commenter fails at `COMMIT` with `OC000`
  (`40001`). Scripted in `sql/07-fk-race.md`.

- **Connection limits in the finale.** Each regional cluster accepts 100 new
  connections/s (burst of 1,000) and 10,000 total. Every Lambda environment
  holds one connection, so load has to ramp up rather than slam in.

## Prep (what this repo needs)

- `demo/`: the polished multi-Region app (CDK, Lambda API, React UI, load
  generator) with an idempotent `deploy.sh`
- `events/`: Rust CLI that tails the CDC Kinesis stream and shows lag (step 10)
- `schema.sql` for step 1, and a reset script
- `stage/`: the demo-it script, generated from TALK.md
- Step 10's Kinesis stream + CDC service role: set up by hand, not in `demo/`
  (Kinesis must be `ON_DEMAND` with `MaxRecordSizeInKiB` 10240)

## Decisions so far

- Drizzle via `@aws/aurora-dsql-drizzle` (node-postgres + IAM via the DSQL
  connector). `db.transactionWithRetry(...)` for writes. No drizzle-kit
  migrations: all DDL happens in psql, so the audience sees jobs.
- The finale app is pre-deployed, so nothing is deployed live on stage.
- Plain Lambda (not Web Functions) for the API, Node 24, with function URLs.
  The UI is static React on S3 + CloudFront, and the same distribution routes
  `/api/<region>/*` to each Region's function URL (IAM auth + origin access
  control: the talk account strips public function URL policies).
- No AI on stage (2026-11-06). The demo-it script types every command.

## Gotchas

- A failed async index build leaves an `INVALID` index that has to be dropped
  by hand.
- No GIN (believed; confirm), so keep the JSONB index demo on a scalar
  (`metadata->>'category'`), not tag arrays.
- Keep reaction counts out of JSONB or counter columns: hot rows mean OCC
  conflicts that dent the finale's TPS.
- The adapter README's `isolationLevel: "serializable"` example will probably
  fail, since DSQL only accepts `REPEATABLE READ`.
- The 2026-06-09 Drizzle blog post predates the adapter, so don't point the
  audience at it.

## Open questions

- Major version upgrades: anything to show?
- Finale target TPS.
