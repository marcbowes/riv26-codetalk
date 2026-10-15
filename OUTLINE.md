# DAT401 (re:Invent 2026): draft outline

Same title and abstract as 2025 ("Amazon Aurora DSQL: A developer's perspective"),
different talk. 2025 hand-built a transfer API from raw `pg`. 2026 builds a blog
live, using the web console where the visuals matter, psql where the exact
results matter, and AI prompting where the point is writing code with very
little DSQL-specific prompting. It ends with a polished version of the same app
running serverless, multi-Region and at scale.

## Ground rules for AI and other slow steps

Anything that takes a while to finish (an agent task, a restore, an index build) must:

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
| 1 | Create cluster A in the console. Ask the AI to create cluster B. Connect to A with psql and type a small schema by hand (`sql/00-by-hand.sql`: posts + comments). `posts.id` is an identity column: typed without a cache it errors, which opens the sequences-in-a-distributed-database discussion; add `(CACHE 65536)`. | console, AI, psql | Cluster creation takes seconds. Cluster B exists for step 10. Clusters show up as random IDs, so mixing them up later is believable. |
| 2 | Insert posts and a comment, by hand. Insert one post from pane B: its id is 65537, because each connection reserved its own block of ids. | psql ×2 | Merge with 3 if short on time. |
| 3 | Recent posts; comments for a post. Then drop the toy schema and `\i sql/01-schema.sql` (users, slugs, bodies, JSONB metadata) plus `02-first-rows.sql`. | psql | Very little pasting in this talk: type the small things, `\i` the real schema. |
| 4 | **Background AI task:** Drizzle API (ListPosts, GetPost, ListComments, AddComment, ...) with a CLI and a server. The agent starts the server and checks it with the CLI. | AI | Longest task, so start it early. It must finish by step 8. |
| 5 | **Background AI task:** generate sample data. Then `EXPLAIN ANALYZE VERBOSE` on the step-3 queries: plan plus DPU cost. | AI, psql | Size the data so the load finishes in ~2 min and the plans are clearly bad. |
| 6 | Add the missing indexes (e.g. `posts(published_at)` for the recent-posts sort). | psql | `CREATE INDEX ASYNC` → job in `sys.jobs` → `wait_for_job` → EXPLAIN again. |
| 7 | FK race: two psql sessions side by side. A deletes a post while B comments on it, and both commit, leaving an orphan. Add the FK `NOT VALID`; validation fails on the orphan. Clean up, validate (a job), and re-run the race. | psql ×2 | Good teaching moment: why both commit without the FK (no write-write conflict) and what the FK changes under OCC. |
| 8 | Step 4 is done: show the API working via the CLI. **Background AI task:** an HTML page that uses the API, run locally. | AI | Start the HTML task here; show the page after step 9. |
| 9 | Query posts by category from `metadata`, then add the expression index on `metadata->>'category'`. Show `->` vs `->>` missing the index. | psql | See the backfill note under Risks. |
| 10 | "Let's set up CDC." In the console, notice there are two clusters. The extra one costs nothing (CloudWatch: zero DPU, so it scales to zero). Clean it up anyway, but delete the **wrong** one. Act alarmed. PITR in the console brings it back. | console | **PITR not shipped yet.** See the PITR notes under Risks. |
| 11 | Set up a CDC stream in the console. A pre-built CLI tool tails the events. Add comments (from the HTML page) and watch them arrive. | console, CLI | Kinesis stream and CDC role set up by hand (not part of `demo/`). Point CDC at the restored cluster. |
| 12 | Multi-Region, explained verbally while walking through setup in the console. | console | Just show the flow; the real one is already built. |

### 35:00–40:00 The polished app

The same blog, polished and already deployed (`demo/`): a React page served
from S3 + CloudFront, calling a Lambda API in each of two Regions, backed by a
multi-Region cluster. Write in one Region and read it in the other right away.

### 40:00–45:00 Wow: load

Generate load (`demo/loadgen`). Lambda and DSQL scale out automatically with
no intervention. Live TPS.

## Risks and things to settle

- **Timing.** Twelve steps in 30 minutes is about 2.5 minutes each. The rough
  estimate comes to ~32. Easiest cuts: merge 2 and 3, and keep 12 to about a minute.
  There's no slack for Q&A.
- **PITR (step 10):**
  - Can you restore a cluster that has been *deleted*?
  - Deletion protection is on by default for clusters created through the API
    (and probably the console), so the "oops" needs a disable step first. That
    can be framed as "this is why it exists".
  - Restore duration: if it's slow, do step 12 during the restore, then step 11.
  - Does the restored cluster get a new endpoint? If so, the step-4 server and
    the step-8 page need restarting with the new endpoint, and CDC goes on the
    restored cluster.
  - CDC `CreateStream` requires the cluster to be `ACTIVE`. Connect to an idle
    cluster first to wake it.
- **Per-transaction limits (3,000 rows, 10 MiB):**
  - Sample data (step 5) has to load in batches.
  - A metadata backfill in step 9 (`UPDATE` across all posts) would hit the limit.
  - Simplest fix: put `metadata` in the step-1 schema and have step 5's
    generator fill it in. Step 9 then says "we've been storing front matter
    all along" and goes straight to querying.
- **AI knowing the adapter.** `@aws/aurora-dsql-drizzle` is from Aug 2026 and
  probably newer than the model's training data. Install the DSQL agent skill
  beforehand; that's a legitimate part of the story. Then rehearse each prompt
  until it reliably works. Keep a snapshot of good output for each AI step.
- **Concurrency between steps.** Step 7's deletes and step 10's cluster deletion
  happen while AI-built things are running. Make sure step 4's self-check is
  finished before step 7 starts deleting rows.
- **FK race specifics (step 7).** Settled by the FK docs: with the FK in place,
  the deleter commits and the commenter fails at `COMMIT` with `OC000`
  (`40001`). Scripted in `sql/07-fk-race.md`.

- **Connection limits in the finale.** Each regional cluster accepts 100 new
  connections/s (burst of 1,000) and 10,000 total. Every Lambda environment
  holds one connection, so load has to ramp up rather than slam in.

## Prep (what this repo needs)

- `demo/`: the polished multi-Region app (CDK, Lambda API, React UI, load
  generator) with an idempotent `deploy.sh`
- `events/`: Rust CLI that tails the CDC Kinesis stream and shows lag (step 11)
- `schema.sql` for step 1, and a reset script
- `prompts/`: the exact, rehearsed prompts for steps 1, 4, 5 and 8, plus a
  fallback snapshot for each
- Step 11's Kinesis stream + CDC service role: set up by hand, not in `demo/`
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
- AI on stage: Kiro or Claude Code. Either works.

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

- PITR: API, restore time, whether it can restore a deleted cluster, and what
  happens to the endpoint. (Marc to share.)
- Major version upgrades: anything to show?
- Finale target TPS.
