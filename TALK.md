# Amazon Aurora DSQL: A developer's perspective (DAT401, re:Invent 2026)

The full talk, written out. It's two things at once:

- **The presenter's script:** what to say, what to type, what to point at.
- **A follow-along guide:** if you couldn't be in the room, you can run every
  step yourself from this page with an AWS account and this repo.

The talk builds a blog (users, posts, comments) on Aurora DSQL in 30 minutes,
then shows a polished version of it running serverless in two Regions at
20,000 requests per second. Most of what we type is ordinary PostgreSQL, on
purpose. The interesting parts are the places where DSQL behaves differently,
and they're called out as you go:

> **★ DSQL difference.** Boxes like this mark what DSQL does that a
> PostgreSQL server doesn't. If you only read these, you get the talk.

<details>
<summary><b>Presenter notes</b> look like this (collapsed on GitHub)</summary>

Timing, staging, and what to do if something goes wrong.
</details>

---

## The six things to take home

1. **There is no database server to run.** A cluster exists seconds after
   you ask for one. There are no instances to size, no versions to patch, no
   failover to configure. You pay for the work your queries do, measured in
   DPUs (DSQL's billing unit), plus storage. An idle cluster costs only its storage.
2. **It's PostgreSQL you already know.** You use psql, SQL, Drizzle,
   `EXPLAIN`, B-tree and expression indexes, foreign keys, JSONB and identity
   columns. Your skills and tools carry over.
3. **It's distributed, and that shows in a few honest places:**
   - Concurrency is optimistic: a conflict surfaces at `COMMIT`, so retry
     the whole transaction.
   - Sequences need a cache.
   - Schema changes on big tables run as background jobs.
   - A transaction changes at most 3,000 rows.

   Each of these is something to learn once, not something to work around.
4. **You can see what a query costs.** `EXPLAIN ANALYZE VERBOSE` ends with
   a per-statement DPU estimate. Indexing a query in this talk takes it from
   about 190 DPU to 0.04.
5. **Multi-Region is active-active with strong consistency.** Both Regions
   read and write. A write in one is visible on the very next read in the
   other. There's no replication lag to design around and no failover to run.
6. **Serverless end to end scales without intervention.** Lambda in front
   of DSQL went from idle to 20,000 requests per second, with no
   provisioning, pool tuning or capacity plan.

---

## Run of show

| Time | Part |
|---|---|
| 0:00–5:00 | Intro: what DSQL is, the agenda |
| 5:00–35:00 | Build it: steps 1–12 (console, psql, AI agent) |
| 35:00–40:00 | The polished app, in two Regions |
| 40:00–45:00 | Scale: 20,000 requests per second |

## Following along

You need:
- **An AWS account.** The examples use **us-west-2**, and the multi-Region
  parts also use us-east-1, with us-east-2 as the witness.
- **AWS CLI v2** (with `aws dsql`), **psql** (16 or newer), **jq**, and this repo:
  `git clone https://github.com/marcbowes/riv26-codetalk && cd riv26-codetalk`
- **Optional: an AI coding agent with the Aurora DSQL skill** (steps 1, 4, 8):
  - Kiro: the [Aurora DSQL power](https://kiro.dev/launch/powers/add/?name=amazon-aurora-dsql).
  - Claude Code: `/plugin marketplace add awslabs/agent-plugins`, then
    `/plugin install databases-on-aws@agent-plugins-for-aws`.
  - Others: `npx skills add https://github.com/awslabs/agent-plugins --skill dsql`.
  - Every AI step also has a no-agent alternative.
- **Optional: Rust** (`cargo`), to build the CDC viewer in `events/`.

Cost: steps 1–12 cost cents. The polished app (`demo/`) runs CloudFront,
Lambda, two clusters and an EC2 host. Tear it down with `demo/destroy.sh`
when you're done. [Cleaning up](#cleaning-up) covers everything.

```sh
export AWS_REGION=us-west-2      # and AWS_PROFILE if you use one
```

---

## 0:00–5:00 · Intro

Two years ago, if you wanted a database you didn't have to run, you used
DynamoDB. It's serverless, it scales to anything, and there are no instances.
But you gave up the relational model to get that: joins, transactions across
tables, constraints, a query planner, SQL.

Aurora DSQL is the answer to "why not both?" It's PostgreSQL-compatible, so
you get the relational model and the tools you already use. And it's serverless
the way DynamoDB is: no instances, no capacity to choose, scale out with load
and down to zero without it. It's also active-active across Regions.

Last year this talk spent 45 minutes on connection pools and retry loops.
This year we skip all that. We'll build a blog: a small schema, some queries,
an API written by an AI agent, and a web page. While we do, I'll show you the
handful of places where a distributed database behaves differently from the
PostgreSQL on your laptop, and why those differences are good news. Then we'll
see the finished app in two Regions, and throw 20,000 requests a second at it.

<details>
<summary>Presenter notes</summary>

- About two slides. The agenda slide mirrors the run-of-show table.
- Have open, logged in:
  - the DSQL console (us-west-2);
  - five terminals: psql A, psql B, Kiro ×2, events;
  - the demo site;
  - the CloudWatch dashboard.
- Ran `demo/preflight.sh` about 30 minutes ago and it said Ready.
</details>

---

## 5:00–35:00 · Build it

### Step 1 · Two clusters, a connection, a schema · 5:00–9:00

**Create a cluster in the console.** In Aurora DSQL in us-west-2, choose
**Create cluster** → single-Region → **Create**.

There's no instance size, no storage to provision, no engine version, no
maintenance window. It's ready in about the time it took to read this
sentence. We'll call it cluster A.

**Create a second one with an AI agent.** I'll ask my agent for another cluster.
It has the DSQL skill installed, which is the only DSQL-specific setup in this
whole talk:

```
Create a new Aurora DSQL cluster in us-west-2 and tell me its endpoint.
```

> No agent? `aws dsql create-cluster --region us-west-2` does the same.

That's cluster B. Remember it; it comes back later.

**Connect with psql.** Notice what's missing:

```sh
export PGHOST=<cluster-A-id>.dsql.us-west-2.on.aws PGUSER=admin PGDATABASE=postgres PGSSLMODE=require
export PGPASSWORD="$(aws dsql generate-db-connect-admin-auth-token --hostname $PGHOST --expires-in 3600)"
psql
```

There's no password. That `PGPASSWORD` is a short-lived token, signed with my
AWS credentials. Who can connect is an IAM question: no secrets to rotate,
nothing in a config file.

> **★ DSQL difference: authentication is IAM.** Tokens are generated
> locally from your AWS credentials and expire. Database roles map to IAM
> roles with `AWS IAM GRANT`. The DSQL connectors for node-postgres, Postgres.js,
> JDBC, Python, Go, Rust, .NET, Ruby and PHP do this for you.

**A small schema, typed by hand.** Posts first, with an ordinary integer
primary key, the way you'd write it anywhere:

```sql
CREATE TABLE posts (
  id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  title text NOT NULL,
  created_at timestamptz DEFAULT now()
);
```
```
ERROR:  identity column is not supported without an explicit cache size.
        please define CACHE greater than or equal to 65536 or equal to 1
```

This is the first place the distributed part shows. A sequence is one shared
counter. In a database where many machines handle your inserts at once, you
have to say how they share it:

- **`CACHE 65536`**: each connection reserves a block of IDs and hands them
  out locally. No coordination per insert, but IDs have gaps and don't follow
  commit order.
- **`CACHE 1`**: every ID comes from the shared counter. Dense, but every
  insert goes through that one counter.

(That's also why `serial` doesn't exist here.) For posts, blocks are fine:

```sql
CREATE TABLE posts (
  id bigint GENERATED ALWAYS AS IDENTITY (CACHE 65536) PRIMARY KEY,
  title text NOT NULL,
  created_at timestamptz DEFAULT now()
);
```

Comments are the high-write table. They get UUIDs, which need no counter at all:

```sql
CREATE TABLE comments (
  id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  post_id bigint NOT NULL,
  body text NOT NULL,
  created_at timestamptz DEFAULT now()
);
```

> **★ DSQL difference: sequences make you choose.** Identity columns and
> sequences work, but you pick the trade-off explicitly: throughput (cache)
> or density (`CACHE 1`). UUIDs avoid the question.

<details>
<summary>Presenter notes</summary>

- Typed, not pasted: [`sql/00-by-hand.sql`](sql/00-by-hand.sql) is the cheat sheet.
- Open psql in pane B now too (same exports). Step 2 uses it, and so does step 7.
- Note whether the console defaulted deletion protection on (it matters in step 10).
</details>

### Step 2 · Some rows · 9:00–10:30

```sql
INSERT INTO posts (title) VALUES ('Hello, DSQL') RETURNING id;      -- 1
INSERT INTO posts (title) VALUES ('Second post') RETURNING id;      -- 2
INSERT INTO comments (post_id, body) VALUES (1, 'First!');
```

Now the same thing from a **second psql session** (pane B):

```sql
INSERT INTO posts (title) VALUES ('From pane B') RETURNING id;      -- 65537
```

That's the cache made visible. Pane B's connection reserved its own block of
65,536 IDs, starting where pane A's ended. No coordination between them, and
also no promise that IDs follow time order.

### Step 3 · Queries · 10:30–12:00

The two queries a blog needs:

```sql
SELECT id, title, created_at FROM posts ORDER BY created_at DESC LIMIT 10;

SELECT p.title, c.body, c.created_at
FROM posts p JOIN comments c ON c.post_id = p.id;
```

A join, an `ORDER BY`, a `LIMIT`: nothing to learn. That's the point.

Now the real schema: users, slugs, post bodies, and a JSONB column for front
matter. This is the one thing I'll load from a file, not type:

```sql
DROP TABLE comments, posts;
\i sql/01-schema.sql
\i sql/02-first-rows.sql
```

[`sql/01-schema.sql`](sql/01-schema.sql) is plain PostgreSQL DDL: UUID keys, a
`UNIQUE` username and slug, and `metadata jsonb` for each post's category, tags
and reading time.

### Steps 4 and 5 · An API by AI, a million rows, and a bad plan · 12:00–17:00

These two run at the same time. The API takes a few minutes, so I'll start it
and come back.

**Step 4: ask the agent for an API** (background):

```
In ./blog, build a TypeScript API for my blog on Aurora DSQL using Drizzle ORM.
The cluster endpoint is <cluster-A-endpoint>; connect as admin. The tables already
exist: read the schema from the database.

Operations: ListPosts (newest first, with author), GetPost (with its comments
and each commenter's username), CreatePost, AddComment.

Give me an HTTP server and a small CLI that calls it over HTTP. Start the server
and use the CLI to check that every operation works.
```

There's nothing DSQL-specific in that prompt beyond the word "DSQL". The skill
supplies the rest: what it should come back with is the
[DSQL adapter for Drizzle](https://github.com/awslabs/aurora-dsql-orms/tree/main/node/drizzle),
`@aws/aurora-dsql-drizzle`, with IAM auth through the connector and
`db.transactionWithRetry(...)` around every write. I'll explain why that retry
matters in step 7.

> No agent? [`demo/api`](demo/api) is a finished version of this API, written for
> the same schema:
> ```sh
> cd demo && npm install
> CLUSTER_ENDPOINT=$PGHOST DB_USER=admin npm run dev:api     # http://localhost:3000
> ```

**Step 5: a million rows.** A blog with three posts is easy to make fast, so
let's load a real one. In a second terminal:

```sh
PGHOST=$PGHOST ./sql/05-load-sample-data.sh
```
```
Loading into <cluster-A>.dsql.us-west-2.on.aws (8 workers, 3000 rows per transaction)
  users          1000 rows in   1s
  posts        100000 rows in   2s
  comments    1000000 rows in  10s
Done in 15s
```

That's 1.1 million rows in 15 seconds, generated inside the database with
`generate_series`, so nothing big crosses the network. It started life as the
script my agent wrote in rehearsal. Look at the batch size: 3,000 rows per
transaction.

> **★ DSQL difference: transactions are bounded.** A transaction can modify
> at most 3,000 rows and 10 MiB, and can run for at most 5 minutes. Bulk loads
> batch, which is good practice anyway. Here, 8 batches run in parallel.

**Now look at a query plan.** The post page lists a post's comments:

```sql
SELECT post_id FROM comments LIMIT 1 \gset
EXPLAIN ANALYZE VERBOSE
SELECT body, created_at FROM comments WHERE post_id = :'post_id' ORDER BY created_at;
```
```
 Sort  (... rows=10 ...)
   Sort Key: comments.created_at
   ->  Full Scan (btree-table) on public.comments  (... actual time=0.653..1030.771 rows=10 loops=1)
             Rows Removed by Filter: 999991
 Execution Time: 1030.908 ms
 Statement DPU Estimate:
   Compute: 1.03405 DPU
   Read: 189.87274 DPU
   Write: 0.00000 DPU
   Total: 190.90679 DPU
```

It read a million rows to return ten. That's an ordinary PostgreSQL plan, but
look at the bottom: **Statement DPU Estimate**. A DPU is what DSQL bills you
for. This plan tells you what the query *costs*, not just how long it takes.
This one costs about 191 DPU every time someone opens a post.

> **★ DSQL difference: the plan shows the price.** `EXPLAIN ANALYZE VERBOSE`
> ends with an estimated DPU cost per statement, broken down into compute,
> read, write and multi-Region write. You can see a query's bill before it
> reaches production.

The home page query is the other one to check:

```sql
EXPLAIN ANALYZE VERBOSE
SELECT title, created_at FROM posts ORDER BY created_at DESC LIMIT 20;
```
That's a scan of all 100,000 posts plus a top-N sort: about 216 ms and 121 DPU.

<details>
<summary>Presenter notes</summary>

- Start step 4 first, then step 5. The loader finishes long before the agent.
- Step 4 must be finished before step 7 starts deleting rows.
- `\gset` must be on its own line; pasting two lines at once breaks it.
- First dry run: Kiro took about 11 minutes for the API. It needed a nudge
  toward `@aws/aurora-dsql-drizzle`, and its CLI called the database directly
  (hence "calls it over HTTP" in the prompt).
</details>

### Step 6 · Indexes, without downtime · 17:00–20:00

The comments index first. `post_id` to find them, `created_at` so they come back
already in order:

```sql
CREATE INDEX ASYNC comments_post_idx ON comments (post_id, created_at);
```
```
           job_id
----------------------------
 bk7zsxu5xjh6pgsaskjqxpb5ty
```

It returned instantly with a job ID. The index builds in the background, on a
table that keeps taking writes:

```sql
SELECT job_id, status, object_name FROM sys.jobs WHERE job_type = 'INDEX_BUILD';
CALL sys.wait_for_job('bk7zsxu5xjh6pgsaskjqxpb5ty');      -- about 45 s on 1M rows
```

> **★ DSQL difference: schema changes are jobs.** `CREATE INDEX ASYNC`
> (and, in step 7, `ALTER TABLE ASYNC ... VALIDATE CONSTRAINT`) runs as a
> background job you can watch in `sys.jobs`. A million-row table gets a new
> index with no lock and no maintenance window.

Same query, run again (↑ in psql):

```
 Index Scan using comments_post_idx on public.comments  (... actual time=1.658..1.671 rows=10 loops=1)
   Index Cond: (comments.post_id = '...'::uuid)
 Execution Time: 1.717 ms
   Total: 0.04104 DPU
```

From **1,031 ms to 1.7 ms**, and from **191 DPU to 0.04**. The Sort node has
gone too, because the index already returns rows in `created_at` order.

Now the home page. The obvious index:

```sql
CREATE INDEX ASYNC posts_created_at_idx ON posts (created_at);
```

Wait for it, run the `EXPLAIN` again, and it's **exactly the same plan**: a full
scan plus a sort, about 216 ms. The planner ignored our index.

Here's why. Look at how DSQL stores a table:

```
\d+ posts
...
Indexes:
    "posts_pkey" PRIMARY KEY, btree_index (id) INCLUDE (author_id, slug, title, body, metadata, created_at)
...
Access method: btree_table
```

The table *is* its primary-key B-tree: the key, with every other column
included. A secondary index on `created_at` holds just `created_at` and the
key. To return `title`, every hit would need a second lookup into the table.
The planner did the arithmetic and decided one scan was cheaper than many
lookups.

So give the index what the query needs:

```sql
CREATE INDEX ASYNC posts_recent_idx ON posts (created_at) INCLUDE (title);
```
```
 Limit  (... actual time=0.808..0.850 rows=20 loops=1)
   ->  Index Only Scan Backward using posts_recent_idx on public.posts
 Execution Time: 0.894 ms
   Total: 0.04336 DPU
```

**216 ms to 0.9 ms.** It's an index-only scan, read backwards for the `DESC`,
so there's no sort and no lookups.

> **★ DSQL difference: covering indexes matter.** Every table is stored by
> its primary key, so a secondary index that doesn't cover the query costs a
> lookup per row. `INCLUDE` the columns you return. And there are no planner
> knobs to reach for instead (`SET enable_sort` is "not supported"): you fix
> plans with indexes.

```sql
DROP INDEX posts_created_at_idx;
```

### Step 7 · A race, and foreign keys · 20:00–24:00

Two psql sessions side by side. The race: one person deletes a post at the
same moment someone else comments on it.

```sql
-- both panes, one line at a time
SELECT id AS post_id FROM posts WHERE slug = 'hello-dsql' \gset
SELECT id AS user_id FROM users WHERE username = 'raluca' \gset
```

| | Pane A | Pane B |
|---|---|---|
| 1 | `BEGIN;` | |
| 2 | `DELETE FROM posts WHERE id = :'post_id';` | |
| 3 | | `BEGIN;` |
| 4 | | `INSERT INTO comments (post_id, user_id, body) VALUES (:'post_id', :'user_id', 'Nice post!');` |
| 5 | `COMMIT;` → ok | |
| 6 | | `COMMIT;` → ok |

Both committed:

```sql
SELECT id, body FROM comments WHERE post_id = :'post_id';   -- 2 rows, on a post that doesn't exist
```

DSQL uses **optimistic concurrency control**. Transactions don't take locks
while they run. At commit, DSQL checks whether anything you wrote was also
written by a transaction that committed in the meantime. A wrote the post, B
wrote a comment: different rows, no conflict, both commit. We've made orphans.

This is what foreign keys are for. On a table that already has a million rows,
you add the key in two steps:

```sql
ALTER TABLE comments ADD CONSTRAINT comments_post_fk
  FOREIGN KEY (post_id) REFERENCES posts (id) ON DELETE CASCADE NOT VALID;

ALTER TABLE ASYNC comments VALIDATE CONSTRAINT comments_post_fk;   -- returns a job_id
CALL sys.wait_for_job('<job_id>');                                 -- succeeded = f
SELECT status, details FROM sys.jobs WHERE job_id = '<job_id>';
--  failed | insert or update on table "comments" violates foreign key constraint "comments_post_fk"
```

`NOT VALID` means "enforce this for new writes from now on". It's already
working: inserting a comment for a made-up post now fails with error 23503.
Validation checks the rows that are already there, as another background job,
and it found our orphans. Clean them up and validate again:

```sql
DELETE FROM comments WHERE post_id = :'post_id';
ALTER TABLE ASYNC comments VALIDATE CONSTRAINT comments_post_fk;
CALL sys.wait_for_job('<job_id>');                                 -- succeeded = t (about 40 s on 1M rows)
```

Now the same race again. Put the post back, reconnect pane B (`\c`), and
re-run the two `\gset` lines in both panes:

```sql
INSERT INTO posts (author_id, slug, title, body)
SELECT id, 'hello-dsql', 'Hello, DSQL', 'again' FROM users WHERE username = 'marc';
```

| | Pane A | Pane B |
|---|---|---|
| 1 | `BEGIN;` | |
| 2 | `DELETE FROM posts WHERE id = :'post_id';` | |
| 3 | | `BEGIN;` |
| 4 | | `INSERT INTO comments ...;` → ok |
| 5 | `COMMIT;` → ok | |
| 6 | | `COMMIT;` → `ERROR: change conflicts with another transaction (OC000)` |

B's insert passed its check: in B's snapshot the post still exists. But at
commit, DSQL treats the referenced post as if B had read it `FOR KEY SHARE`. A
deleted it, so B loses with a serialization error (SQLSTATE `40001`). No locks
were taken while either transaction ran, and you still can't create an orphan.

> **★ DSQL difference: optimistic, not locking.** Conflicts surface at
> `COMMIT` as `40001`, and the fix is always the same: **retry the whole
> transaction**. That's what `transactionWithRetry` in the Drizzle adapter does
> (the connectors have the same). Foreign keys work this way too: checked
> against your snapshot, enforced at commit, with no locks while the
> transaction runs.

<details>
<summary>Presenter notes</summary>

- **Why reconnect pane B:** it still has the schema from before the
  `ALTER TABLE`. Its next statement on `comments` would fail with
  `schema has been updated by another transaction (OC001)`, a schema
  conflict rather than the data conflict (OC000) we want to show. If it
  happens anyway, it's a fine teaching moment.
- Type `\gset` lines one at a time; don't paste them as a pair.
- Full script with expected output: [`sql/07-fk-race.md`](sql/07-fk-race.md).
</details>

### Step 8 · The API works, now give it a page · 24:00–26:00

By now the agent has finished the API. Over HTTP:

```sh
curl -s 'localhost:3000/posts?limit=3' | jq .
```

Then one more background task:

```
Build a single-page web UI for the blog API running on localhost:3000: a home
page of recent posts, a post page with its comments, and a form to add a
comment. Serve it from the same server and give me the URL.
```

> No agent? [`demo/web`](demo/web) works against the local API:
> `cd demo && VITE_API_URL=http://localhost:3000 npm run dev:web`

### Step 9 · JSONB, and an index on an expression · 26:00–29:00

Every post has front matter in a JSONB column:

```sql
SELECT title, metadata FROM posts LIMIT 2;
```
```
 A practical guide to async indexes, revisited | {"seo": {...}, "tags": ["dsql", "postgres"], "category": "databases", "readingMinutes": 7}
```

The category page:

```sql
EXPLAIN ANALYZE VERBOSE
SELECT title, created_at FROM posts
WHERE metadata->>'category' = 'databases'
ORDER BY created_at DESC LIMIT 20;
```

That's a full scan: 87,570 rows filtered out, about 235 ms and 121 DPU.
PostgreSQL's answer is an index on the expression:

```sql
CREATE INDEX ASYNC posts_category_idx ON posts ((metadata->>'category'), created_at);
CALL sys.wait_for_job('<job_id>');                     -- about 11 s
```
```
 Limit  (... actual time=2.108..2.137 rows=20 loops=1)
   ->  Index Scan Backward using posts_category_idx on public.posts
         Index Cond: ((posts.metadata ->> 'category'::text) = 'databases'::text)
 Execution Time: 2.180 ms
   Total: 0.06800 DPU
```

**235 ms to 2.2 ms.** One catch: the query has to use *exactly* the indexed
expression. `->` returns JSON rather than text, so this version doesn't match
the index and is a full scan again:

```sql
EXPLAIN ANALYZE SELECT title FROM posts WHERE metadata->'category' = '"databases"' LIMIT 20;
```

And one boundary. "Posts with *any* tag `dsql`" works (`metadata->'tags' ? 'dsql'`),
but it's always a full scan. In PostgreSQL you'd add a GIN index, and DSQL doesn't
support those (`USING not supported for CREATE INDEX`). If you need to filter on
values inside an array at scale, pull a single value out into an expression
index, or model it as rows.

> **★ DSQL difference: JSONB is compressed by default.** You never asked for
> compression, but `pg_attribute.attcompression` is `l` (lz4) on `title`,
> `body` and `metadata`. Large text and JSON values are stored compressed.

### Step 10 · Scale to zero, and an accident · 29:00–33:00

I want to show you change data capture, but first let's tidy up. In the
console there are **two** clusters. Cluster B, the one the agent made, has
never been used. Its monitoring shows nothing at all: no DPUs, no instance
ticking over.

> **★ DSQL difference: idle costs nothing.** There's no instance to keep
> warm. A cluster that isn't doing work bills only for its storage, which for
> cluster B is nothing.

We don't need it, so let's delete it. I select it, choose Delete, type the
confirmation...

...and that was cluster A. The one with our blog in it.

*(Point-in-time restore: the commands and timings for this part will be added
once the feature is available. The story: restore cluster A to a minute before
the delete, from the console, and carry on.)*

<details>
<summary>Presenter notes</summary>

- Deletion protection is on by default for API-created clusters. Either
  disable it on A before the talk, or use it: "this is what it's for".
- Restore details are still to come: API, duration, endpoint changes.
  If the endpoint changes, re-export `PGHOST`, restart the step-4 server,
  and point CDC at the restored cluster.
- CDC needs an `ACTIVE` cluster. Run a query against the restored one first.
- **Following along:** don't delete your cluster unless you mean to.
</details>

### Step 11 · Change data capture · 33:00–34:00

Every committed change, streamed to Kinesis. First, start the viewer (it reads
from the latest position, so it shows only new changes):

```sh
./events/target/release/events --stream <kinesis-stream> --table comments
```

Then, in the console: cluster A → create a CDC stream → target the Kinesis
stream → create. Now add a comment from the step-8 page:

```
commit       op     table    tx           cdc kinesis    here   total  row
12:53:25.099 INSERT comments ea2akesm   102ms    53ms   225ms   380ms  "Great post!" by user ee0a7cf3… on post d9bd532b…
```

*(Illustrative line from [`events/samples/blog.jsonl`](events/samples/blog.jsonl).)*
Each record carries:
- the commit timestamp;
- the transaction ID;
- the operation: `c` (create), `u` (update) or `d` (delete);
- the full row after the change, or for a delete just the primary key.

The four lag columns break the trip down: commit → CDC → Kinesis → this laptop.

> **★ DSQL difference: CDC is built in and doesn't load the database.**
> It's a managed part of the cluster: create a stream, point it at Kinesis
> (and from there Lambda, or Firehose to S3, Redshift or OpenSearch), and
> committed changes flow. It's designed to have zero impact on the database
> workload.

<details>
<summary>Following along: create the stream yourself</summary>

The Kinesis stream must be **on-demand with 10 MiB records**. CDC splits large
rows into fragments of up to 10 MiB.

```sh
aws kinesis create-stream --stream-name dat401-cdc \
  --stream-mode-details StreamMode=ON_DEMAND --max-record-size-in-ki-b 10240
```

DSQL writes to it through a service role that trusts `dsql.amazonaws.com`. The
console can create the role for you; the CLI steps are in
[Getting started with CDC streams](https://docs.aws.amazon.com/aurora-dsql/latest/userguide/cdc-setup.html).
Then:

```sh
aws dsql create-stream --cluster-identifier <cluster-A-id> \
  --target-definition kinesis={streamArn=<stream-arn>,roleArn=<role-arn>} \
  --ordering UNORDERED --format JSON
```
</details>

### Step 12 · Multi-Region · 34:00–35:00

Last console stop: **Create cluster → Multi-Region**. You choose the two Regions
that take reads and writes, us-west-2 and us-east-1, and a **witness** Region,
us-east-2. The witness stores a limited window of encrypted transaction logs,
which improves the cluster's durability and availability if a Region has a
problem. It has no endpoint.

I'm not going to click Create. That cluster already exists, and the finished
blog runs on it.

> **★ DSQL difference: active-active, strongly consistent.** There are two
> endpoints, both read-write, onto one logical database. A commit waits for
> the other Region, so writes pay a cross-Region round trip. Reads stay local,
> and every read sees every committed write, wherever it was made.

---

## 35:00–40:00 · The polished app

The same blog, finished ([`demo/`](demo)):
- a React page on S3 and CloudFront;
- a Lambda function in each Region, running the same Hono + Drizzle code we
  ran locally;
- one multi-Region DSQL cluster, peered between us-west-2 and us-east-1.

The page has one column per Region:
- **Round trip:** browser to that Region and back. From Las Vegas, us-west-2
  is closer.
- **In Aurora DSQL:** database time only, about the same in both Regions,
  because each reads from its local endpoint.

Open a post; with "Keep both columns on the same page" on, both columns follow.
Write a comment in the us-west-2 column, and watch us-east-1:

> *Posted in us-west-2, read here N ms after it was saved.*

That's the next read in the other Region, not a replication delay we're
waiting out. In our measurements, a write spent about **80–190 ms** in the
database (the commit crosses the country) and a read about **11–14 ms**. For a
blog that's 90% reads, that's the right trade-off.

> **★ DSQL difference: no replication lag to code around.** The
> "read-your-writes in another Region" problem doesn't exist. There's no
> primary to fail over to, and no stale replica to route around.

<details>
<summary>Presenter notes and following along</summary>

- Deploy with `cd demo && ./deploy.sh` (about 10 minutes the first time; it's
  idempotent). Check readiness with `./preflight.sh`, which also warms both
  APIs so the first click isn't a cold start.
- Architecture: one CloudFront distribution serves the page from S3, and
  routes `/api/<region>/*` to an IAM-authenticated Lambda function URL in each
  Region (only the distribution can call them). Details in
  [`demo/README.md`](demo/README.md).
</details>

---

## 40:00–45:00 · Scale

Everything so far has been one person clicking. Here's 20,000 requests a
second. The load generator runs on an EC2 host in us-west-2, and its display
shows here:

```sh
cd demo/loadgen
./target/release/loadgen --run-on-ec2 --rps 20000 --title "DAT401"
```

The traffic is 90% reads and 10% writes (new comments), split across both
Regions. It ramps over 60 seconds, because every new Lambda execution
environment opens one DSQL connection, and a cluster accepts 100 new
connections a second.

From our rehearsal:

```
SUMMARY  83.0 s of load · open loop, target 20,000 req/s · ramp 60 s · mix 90/10 read/write
Region      requests        2xx  avg TPS  peak TPS  p50 ms  p90 ms  p99 ms   DB p50  DB p99
us-west-2    530,031    530,031    6,386    10,155      47     109     137       18      93
us-east-1    498,281    497,866    5,998    10,366     107     174     324       14      92
all        1,028,312  1,027,897   12,384    20,368      95     128     283       17      92
```

On the dashboard (CloudWatch, a minute behind), watch Lambda invocations and
concurrent environments climb in both Regions, and DSQL transactions and DPUs
follow. Nobody provisioned anything for this:
- no instance sizes;
- no read replicas;
- no connection pool tuning;
- no capacity plan.

It's the same code that ran on my laptop an hour ago. Then stop the load, and
watch both graphs fall to zero.

> **★ DSQL difference: serverless end to end.** Lambda adds environments,
> DSQL adds capacity, and both bill for work done. The database p50 stayed
> about 14–18 ms from the first request to the millionth.

<details>
<summary>Presenter notes</summary>

- Lambda caps synchronous requests at 10 × the Region's concurrency quota per
  second. At the default 1,000 that's 10,000 req/s, which is why us-east-1 shows
  415 throttles (429s) in the run above. [`demo/quotas.sh`](demo/quotas.sh)
  raises both Regions to 10,000.
- The host is patched by the account's weekly SSM job (Tuesdays about
  2:46 PM PST). Leave it running from Tuesday, or start it at least an hour
  before the talk. `preflight.sh` checks its uptime and the next patch time.
- `loadgen --stop-ec2` afterwards.
</details>

---

## Wrap-up

What we did in 45 minutes, and what DSQL did differently:

| We did | DSQL made it different |
|---|---|
| Created a cluster | Seconds; nothing to size, patch or fail over |
| Connected with psql | IAM tokens, no passwords |
| Wrote a schema | Sequences need a cache; UUIDs need nothing |
| Loaded 1.1M rows | 3,000 rows per transaction; batch and parallelize |
| Read a query plan | The plan shows the DPU cost |
| Added indexes | Built as background jobs; covering indexes matter |
| Raced two transactions | Optimistic: conflicts at `COMMIT`; retry the transaction |
| Added a foreign key | Validated as a job; enforced without locks |
| Indexed JSONB | Compressed by default; expression indexes, no GIN |
| Left a cluster idle | It cost nothing |
| Streamed changes | Built-in CDC to Kinesis |
| Wrote in one Region | Read it in the other, immediately |
| Hit it with 20k req/s | Nobody provisioned anything |

Links:
- **Aurora DSQL:** [docs](https://docs.aws.amazon.com/aurora-dsql/latest/userguide/),
  plus the browser playground (no account needed).
- **[Connectors](https://github.com/awslabs/aurora-dsql-connectors)**
  (IAM auth, OCC retry) and the
  **[Drizzle adapter](https://github.com/awslabs/aurora-dsql-orms/tree/main/node/drizzle)**.
- **The DSQL agent skill / Kiro power:** [setup](https://docs.aws.amazon.com/aurora-dsql/latest/userguide/SECTION_aurora-dsql-steering.html).
- **This repo:** every script used today.

---

## Cleaning up

```sh
# Steps 1-12: your clusters (turn off deletion protection first if it's on)
aws dsql update-cluster --identifier <id> --no-deletion-protection-enabled
aws dsql delete-cluster --identifier <id>

# Step 11: the CDC stream, then the Kinesis stream
aws dsql delete-stream --cluster-identifier <cluster-A-id> --stream-identifier <stream-id>
aws kinesis delete-stream --stream-name dat401-cdc

# The polished app, its clusters, and the EC2 host
cd demo && ./destroy.sh
```

## Troubleshooting

- **`\gset: extra argument ... ignored`**: two lines were pasted at once, so
  psql read the second as arguments. Put each `\gset` query on its own line.
- **`schema has been updated by another transaction (OC001)`**: the
  session's cached schema predates a DDL change. Retry, or reconnect (`\c`).
- **`change conflicts with another transaction (OC000)`** (SQLSTATE
  `40001`): an optimistic-concurrency conflict at commit. Retry the whole
  transaction; the connectors and the Drizzle adapter can do it for you.
- **psql can't reconnect after an hour**: the IAM token expired. Generate a
  new `PGPASSWORD`.
- **The loader fails right after creating tables**: freshly created tables can
  briefly return OC001. The loader retries each batch; re-running it is safe.
- **CDC stream impaired with `KINESIS_OVERSIZE_RECORD`**: the Kinesis stream's
  `MaxRecordSizeInKiB` must be 10240.
- **429s from the load generator**: usually Lambda's request-rate limit
  (10 × concurrency per second). Run `demo/quotas.sh`, or send less to that Region
  with `--region-weights`.
- **The planner ignores your index**: check whether the query needs columns
  the index doesn't have (`INCLUDE` them), and whether it uses exactly the
  indexed expression (`->>` vs `->`).
