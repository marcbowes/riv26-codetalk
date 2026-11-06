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
   - The primary key is the physical layout, so choose it for how you read.
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
| 5:00–35:00 | Build it: steps 1–11 (console, psql) |
| 35:00–40:00 | The polished app, in two Regions |
| 40:00–44:00 | Scale: 20,000 requests per second |
| 44:00–45:00 | Wrap-up |

## Following along

You need:
- **An AWS account.** The examples use **us-west-2**, and the multi-Region
  parts also use us-east-1, with us-east-2 as the witness.
- **AWS CLI v2** (with `aws dsql`), **psql** (16 or newer), **jq**, and this repo:
  `git clone https://github.com/marcbowes/riv26-codetalk && cd riv26-codetalk`
- **Optional: Node.js**, to run the API from step 4 yourself (`demo/api`).
- **Optional: Rust** (`cargo`), to build the CDC viewer in `events/`.

Cost: steps 1–11 cost cents. The polished app (`demo/`) runs CloudFront,
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
an API, and a web page. While we do, I'll show you the
handful of places where a distributed database behaves differently from the
PostgreSQL on your laptop, and why those differences are good news. Then we'll
see the finished app in two Regions, and throw 20,000 requests a second at it.

<details>
<summary>Presenter notes</summary>

- About two slides. The agenda slide mirrors the run-of-show table.
- Have open, logged in:
  - the DSQL console (us-west-2);
  - Emacs, with `M-x dat401-start` run (see [`stage/`](stage));
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
sentence.

**And one from the terminal.** It's one API call, and this is the cluster we'll
build on:

```sh pane A
aws dsql create-cluster
```
```text
{
    "identifier": "e5ugy6wru5xgmpi4lx4e6sxs54",
    "arn": "arn:aws:dsql:us-west-2:111122223333:cluster/e5ugy6wru5xgmpi4lx4e6sxs54",
    "status": "CREATING",
    "creationTime": "2026-11-06T09:50:11.597000-08:00",
    "encryptionDetails": {
        "encryptionType": "AWS_OWNED_KMS_KEY",
        "encryptionStatus": "ENABLED"
    },
    "deletionProtectionEnabled": true,
    "endpoint": "e5ugy6wru5xgmpi4lx4e6sxs54.dsql.us-west-2.on.aws"
}
```

<!-- stage: highlight identifier -->
That's the cluster's identifier.
<!-- stage: highlight endpoint -->
And that's its endpoint: the identifier, then `.dsql.us-west-2.on.aws`. That's
all psql needs. In rehearsal the cluster was active four seconds later.

**Connect with psql.** Notice what's missing:

```sh pane A
export PGHOST=<cluster-endpoint> PGUSER=admin PGDATABASE=postgres PGSSLMODE=require
```
<!-- stage: clear -->
```sh pane A
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

<!-- stage: open pane B -->
**A second session, for later.** Step 7 needs two. The same environment, then
psql:

```sh pane B paste
export PGHOST=<cluster-endpoint> PGUSER=admin PGDATABASE=postgres PGSSLMODE=require
export PGPASSWORD="$(aws dsql generate-db-connect-admin-auth-token --hostname $PGHOST --expires-in 3600)"
```
```sh pane B
psql
```

**A small schema, typed by hand.** Posts first, with a UUID primary key:

```sql
CREATE TABLE posts (
  id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  title text NOT NULL,
  created_at timestamptz DEFAULT now()
);
```

Why not an ordinary integer ID? Sequences work in DSQL, but a sequence is one
shared counter, and when many machines handle your inserts at once you have to
say how they share it. `CACHE 1` gives dense, ordered IDs at a round trip each.
A big cache (65536 or more) is fast, but leaves gaps, and the IDs don't follow
commit order. A UUID needs no counter at all, so that's what we'll use
everywhere.

Comments are the high-write table. Their IDs are UUIDs too, but the primary
key starts with the *post*:

```sql
CREATE TABLE comments (
  post_id uuid NOT NULL,
  comment_id uuid NOT NULL DEFAULT gen_random_uuid(),
  body text NOT NULL,
  created_at timestamptz DEFAULT now(),
  PRIMARY KEY (post_id, comment_id)
);
```

This is the schema-design choice of the talk. Random keys and sequential keys
pull in opposite directions:

- **Random keys go wide.** Inserts land all over the key space, so the work
  spreads across the whole fleet. That's DynamoDB's partition-key advice, and
  it's why comment IDs are UUIDs.
- **Keys that group things go fast.** DSQL stores a table by its primary key,
  as we'll see in step 6. With `post_id` first, a post's comments sit next to
  each other, and "load the comments for this post" becomes one range read. It
  needs no secondary index.

The price is a hotspot: every comment on one post lands in the same part of
the key space. That's fine for a blog. Comments on a single post never need to
scale without limit; comments across *all* posts do, and they still spread out.

> **★ DSQL difference: sequences make you choose.** Identity columns and
> sequences work, but you pick the trade-off explicitly: `CACHE 1` for dense
> and ordered (a round trip per ID), a big cache for throughput. UUIDs avoid
> the question.

> **★ DSQL difference: the primary key is your physical layout.** Choose it
> for how you read. Random where you need writes to spread, grouped where you
> read related rows together.

<details>
<summary>Presenter notes</summary>

- demo-it types it. [`sql/00-by-hand.sql`](sql/00-by-hand.sql) is the same schema.
- The console cluster is only for show. Nothing uses it until step 9 deletes
  it, so leave its deletion protection on: step 9 turns it off.
- F12s after `create-cluster`: highlight the identifier; highlight the
  endpoint; type the `PGHOST` export (the endpoint is copied from the output
  and pasted, and highlighted too); clear; the token and `psql`. There's
  nothing to copy by hand.
- Pane B: F12 opens it, F12 pastes the exports, F12 runs `psql`. It's ready
  for step 7.
- If psql says the cluster isn't ready, wait a few seconds and run `psql` again.
- Sequences are talked about, not shown: for the numbers (CACHE 1: a 3,000-row
  insert took 20.6 s; CACHE 65536: 117 ms), see `sql/00-by-hand.sql`.
</details>

### Step 2 · Some rows · 9:00–10:30

```sql
INSERT INTO posts (title) VALUES ('Hello, DSQL'), ('Second post');
```

Nothing to learn here, and that's the point: it's an `INSERT`.

```sql
SELECT * FROM posts;
```
```text
                  id                  |    title    |          created_at
--------------------------------------+-------------+-------------------------------
 60ddf88c-76a5-4c34-a109-d5f0bef7b656 | Hello, DSQL | 2026-11-06 10:12:31.524811+00
 fa165d6b-7dbf-4e93-b606-c3273f00370f | Second post | 2026-11-06 10:12:31.524811+00
```

The IDs are UUIDs, generated on whichever part of the fleet handled the
insert, with nothing shared to coordinate on. To comment on the first post,
save its ID in a psql variable:

```sql
SELECT id AS post_id FROM posts WHERE title = 'Hello, DSQL' \gset
```
```sql
INSERT INTO comments (post_id, body) VALUES (:'post_id', 'First!');
```

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

[`sql/01-schema.sql`](sql/01-schema.sql) is plain PostgreSQL DDL with the same
choices: UUIDs everywhere, and comments keyed by `(post_id, comment_id)`. It
adds users, a `UNIQUE` username and slug, and `metadata jsonb` for each post's
category, tags and reading time.

### Step 4 · An API, from one prompt · 12:00–13:00

With a schema in place, the application is the easy part. I'll ask Claude Code
for it. It has the Aurora DSQL skill installed, which is the only DSQL-specific
setup here:

```prompt
in ./blog build a typescript api for my blog on aurora dsql w/ drizzle orm. cluster endpoint is in $PGHOST, connect as admin. tables already exist so just read teh schema from the db. ops: list posts (newest first w/ author), get post (with its comments + each comenters username), create post, add comment. give me an http server and a small cli that calls it over http. then start the server and use the cli to chcek every op works
```

*(Typed the way people actually prompt, typos included. The agent doesn't mind.)*

I'm not going to submit that. I did it earlier, so you don't have to wait for
it. There's nothing DSQL-specific in the prompt beyond the word "DSQL": the
skill supplies the rest. Here's the code it led to, as it runs in the finished
app at 35:00. First, the connection:

```ts demo/api/src/db.ts
import { drizzle } from "@aws/aurora-dsql-drizzle";
import { AuroraDSQLPool } from "@aws/aurora-dsql-node-postgres-connector";
...
export function createDb({ host, user, max }: DbOptions) {
  const pool = new AuroraDSQLPool({
    host,
    user,
    max,
    // DSQL closes connections after 60 minutes. Recycle well before that.
    maxLifetimeSeconds: 50 * 60,
...
  return drizzle({ client: pool, schema });
}
```

That's the
[DSQL adapter for Drizzle](https://github.com/awslabs/aurora-dsql-orms/tree/main/node/drizzle),
`@aws/aurora-dsql-drizzle`, on the DSQL connector for node-postgres. The
connector makes a fresh IAM token for each new connection, so there's no
password anywhere: not in the code, not in an environment variable.

Then every write:

```ts demo/api/src/queries.ts
export async function addComment(
...
  return db.transactionWithRetry(
    async (tx) => {
      const [post] = await tx.select({ id: posts.id }).from(posts).where(eq(posts.id, postId));
      if (!post) throw new HttpError(404, "post not found");
...
      const [row] = await tx
        .insert(comments)
        .values({ postId, userId: user.id, body: input.body })
...
    },
    undefined,
    WRITE_RETRY,
  );
}
```

`db.transactionWithRetry(...)` runs the whole transaction again if it loses a
conflict. Why a database would make you do that is step 7.

> Following along: [`demo/api`](demo/api) is this API, written for the same
> schema. To run it against your cluster:
> ```sh
> cd demo && npm install
> CLUSTER_ENDPOINT=$PGHOST DB_USER=admin npm run dev:api     # http://localhost:3000
> ```

<details>
<summary>Presenter notes</summary>

- F12 opens a terminal beside psql, starts Claude Code, and types the prompt.
  The next F12 closes it. Then `db.ts`, then `queries.ts`: the real files, with
  the lines above highlighted.
- The prompt is never submitted: demo-it sends no Enter and no newline (the
  paragraphs are typed as one). **Don't press Enter while Claude Code is on
  screen**: it's in auto mode, so it would start building.
- Kiro built the first version in rehearsal. In the first dry run Kiro took about 11
  minutes. Its first version used plain `drizzle-orm` with the connector and
  needed a nudge toward `@aws/aurora-dsql-drizzle`, and its CLI called the
  database directly (hence "calls it over HTTP" in the prompt).
</details>

### Step 5 · A million rows, and a bad plan · 13:00–17:00

A blog with three posts is easy to make fast, so let's load a real one. In a
second terminal:

```sh
PGHOST=$PGHOST ./sql/05-load-sample-data.sh
```
```
Loading into <cluster-id>.dsql.us-west-2.on.aws (8 workers, 3000 rows per transaction)
  users          1000 rows in   1s
  posts        100000 rows in   2s
  comments    1000000 rows in  10s
Done in 15s
```

That's 1.1 million rows in 15 seconds, generated inside the database with
`generate_series`, so nothing big crosses the network. Look at the batch size
in that first line: 3,000 rows per transaction.

> **★ DSQL difference: transactions are bounded.** A transaction can modify
> at most 3,000 rows and 10 MiB, and can run for at most 5 minutes. Bulk loads
> batch, which is good practice anyway. Here, 8 batches run in parallel.

<!-- stage: close pane shell -->
**Now look at a query plan.** First the home page, the 20 newest posts:

```sql
EXPLAIN ANALYZE VERBOSE
SELECT title, created_at FROM posts ORDER BY created_at DESC LIMIT 20;
```
```
 Limit  (... actual time=208.010..208.015 rows=20 loops=1)
   ->  Sort  (... rows=100001 ...)
         Sort Key: posts.created_at DESC
         Sort Method: top-N heapsort  Memory: 28kB
         ->  Index Only Scan using posts_pkey on public.posts  (... actual time=0.707..192.167 rows=100001 loops=1)
 Execution Time: 208.074 ms
 Statement DPU Estimate:
   Compute: 0.22043 DPU
   Read: 120.54339 DPU
   Write: 0.00000 DPU
   Total: 120.76381 DPU
```

<!-- stage: highlight scan -->
It read all 100,000 posts.
<!-- stage: highlight sort -->
Then it sorted them, to return 20. That's an ordinary PostgreSQL plan.
<!-- stage: highlight time dpu -->
But look at the bottom: **Statement DPU Estimate**. A DPU is what DSQL bills
you for, so this plan tells you what the query *costs*, not
just how long it takes. This one costs about 121 DPU every time someone loads
the home page. We'll fix it in step 6.

> **★ DSQL difference: the plan shows the price.** `EXPLAIN ANALYZE VERBOSE`
> ends with an estimated DPU cost per statement, broken down into compute,
> read, write and multi-Region write. You can see a query's bill before it
> reaches production.

Now the post page, a post's comments, out of a million:

```sql
SELECT post_id FROM comments LIMIT 1 \gset
EXPLAIN ANALYZE VERBOSE
SELECT body, created_at FROM comments WHERE post_id = :'post_id' ORDER BY created_at;
```
```
 Sort  (... actual time=0.473..0.474 rows=14 loops=1)
   Sort Key: comments.created_at
   ->  Index Only Scan using comments_pkey on public.comments  (... actual time=0.374..0.415 rows=14 loops=1)
         Index Cond: (comments.post_id = '00010fdc-b992-0173-c302-f62c7ee40d70'::uuid)
 Execution Time: 0.714 ms
   Total: 0.00724 DPU
```

<!-- stage: highlight time dpu -->
**0.7 ms and under 0.01 DPU, with no index added.**
<!-- stage: highlight scan cond -->
That's the primary key from step 1 paying off: the comments for this post are one contiguous range of
`comments_pkey`. For comparison, in rehearsal with comments keyed by
`comment_id` alone, this same query was a full scan of all million comments:
**1,031 ms and 191 DPU**.

<details>
<summary>Presenter notes</summary>

- `\gset` must be on its own line; pasting two lines at once breaks it.
</details>

### Step 6 · An index, without downtime · 17:00–20:00

The comments don't need an index. The home page does. The obvious one:

```sql
CREATE INDEX ASYNC posts_created_at_idx ON posts (created_at);
```
```
           job_id
----------------------------
 sdtn7gjobfeglgjbysgk3pog4i
```

It returned instantly with a job ID. The index builds in the background, on a
table that keeps taking writes:

```sql
SELECT job_id, status, object_name FROM sys.jobs WHERE job_type = 'INDEX_BUILD';
CALL sys.wait_for_job('sdtn7gjobfeglgjbysgk3pog4i');      -- about 25 s on 100k posts
```

> **★ DSQL difference: schema changes are jobs.** `CREATE INDEX ASYNC`
> (and, in step 7, `ALTER TABLE ASYNC ... VALIDATE CONSTRAINT`) runs as a
> background job you can watch in `sys.jobs`. A big table gets a new index
> with no lock and no maintenance window.

Run the home page `EXPLAIN` again (Ctrl-R in psql):

```sql recall: desc limit
EXPLAIN ANALYZE VERBOSE
SELECT title, created_at FROM posts ORDER BY created_at DESC LIMIT 20;
```

<!-- stage: highlight scan sort -->
It's **exactly the same plan**: a full scan plus a sort, about 208 ms. The
planner ignored our index.

Here's why. Look at how DSQL stores a table:

```sql
\d+ posts
```
```
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
CALL sys.wait_for_job('<job_id>');
```

Then the home page query once more (Ctrl-R again):

```sql recall: desc limit
EXPLAIN ANALYZE VERBOSE
SELECT title, created_at FROM posts ORDER BY created_at DESC LIMIT 20;
```
```
 Limit  (... actual time=0.637..0.673 rows=20 loops=1)
   ->  Index Only Scan Backward using posts_recent_idx on public.posts  (... actual time=0.635..0.670 rows=20 loops=1)
 Execution Time: 0.751 ms
   Total: 0.03857 DPU
```

<!-- stage: highlight time dpu -->
**208 ms to 0.7 ms**, and **121 DPU to 0.04**.
<!-- stage: highlight scan -->
It's an index-only scan, read backwards for the `DESC`: 20 rows read to return
20, with no sort and no lookups.

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
SELECT comment_id, body FROM comments WHERE post_id = :'post_id';   -- 2 rows, on a post that doesn't exist
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
CALL sys.wait_for_job('<job_id>');                                 -- succeeded = t (about 10 s on 1M rows)
```

Now the same race again. Put the post back:

```sql pane A
INSERT INTO posts (author_id, slug, title, body)
SELECT id, 'hello-dsql', 'Hello, DSQL', 'again' FROM users WHERE username = 'marc';
```

Reconnect pane B, so its session picks up the new foreign key:

```sql pane B
\c
```

And look the post up again in both panes, because it has a new ID:

```sql both panes
SELECT id AS post_id FROM posts WHERE slug = 'hello-dsql' \gset
SELECT id AS user_id FROM users WHERE username = 'raluca' \gset
```

| | Pane A | Pane B |
|---|---|---|
| 1 | `BEGIN;` | |
| 2 | `DELETE FROM posts WHERE id = :'post_id';` | |
| 3 | | `BEGIN;` |
| 4 | | `INSERT INTO comments (post_id, user_id, body) VALUES (:'post_id', :'user_id', 'Nice post!');` → ok |
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

### Step 8 · JSONB, and an index on an expression · 24:00–27:00

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
```
 Limit  (... actual time=231.026..231.031 rows=20 loops=1)
   ->  Sort  (... rows=500 ...)
         Sort Key: posts.created_at DESC
         Sort Method: top-N heapsort  Memory: 28kB
         ->  Index Only Scan using posts_pkey on public.posts  (... actual time=0.999..228.691 rows=12441 loops=1)
               Filter: ((posts.metadata ->> 'category'::text) = 'databases'::text)
               Rows Removed by Filter: 87560
 Execution Time: 231.112 ms
   Total: 120.79467 DPU
```

<!-- stage: highlight filter -->
That's a full scan: it read every post and threw 87,560 of them away.
<!-- stage: highlight time dpu -->
About 230 ms and 121 DPU. PostgreSQL's answer is an index on the expression:

```sql
CREATE INDEX ASYNC posts_category_idx ON posts ((metadata->>'category'), created_at);
CALL sys.wait_for_job('<job_id>');                     -- about 11 s
```

The category query again (Ctrl-R):

```sql recall: = 'databases'
EXPLAIN ANALYZE VERBOSE
SELECT title, created_at FROM posts
WHERE metadata->>'category' = 'databases'
ORDER BY created_at DESC LIMIT 20;
```
```
 Limit  (... actual time=2.108..2.137 rows=20 loops=1)
   ->  Index Scan Backward using posts_category_idx on public.posts
         Index Cond: ((posts.metadata ->> 'category'::text) = 'databases'::text)
 Execution Time: 2.180 ms
   Total: 0.06800 DPU
```

<!-- stage: highlight scan cond -->
An index scan on the expression: 20 rows read to return 20.
<!-- stage: highlight time dpu -->
**About 230 ms to 2 ms**, and 121 DPU to 0.07.

One catch: the query has to use *exactly* the indexed expression. `->` returns JSON rather than text, so this version doesn't match
the index and is a full scan again:

```sql
EXPLAIN ANALYZE SELECT title FROM posts WHERE metadata->'category' = '"databases"' LIMIT 20;
```
```
 Limit  (... actual time=0.897..1.214 rows=20 loops=1)
   ->  Index Only Scan using posts_pkey on posts  (... actual time=0.896..1.211 rows=20 loops=1)
         Filter: ((metadata -> 'category'::text) = '"databases"'::jsonb)
         Rows Removed by Filter: 131
```
<!-- stage: highlight scan filter -->

And one boundary. "Posts with *any* tag `dsql`" works (`metadata->'tags' ? 'dsql'`),
but it's always a full scan. In PostgreSQL you'd add a GIN index, and DSQL doesn't
support those (`USING not supported for CREATE INDEX`). If you need to filter on
values inside an array at scale, pull a single value out into an expression
index, or model it as rows.

> **★ DSQL difference: JSONB is compressed by default.** You never asked for
> compression, but `pg_attribute.attcompression` is `l` (lz4) on `title`,
> `body` and `metadata`. Large text and JSON values are stored compressed.

### Step 9 · Scale to zero · 27:00–29:00

Before change data capture, let's tidy up. In the console there are **two**
clusters: the blog's, and the one I made in the console at the very start.
That one has never been used. Its monitoring shows nothing at all: no DPUs, no
instance ticking over.

> **★ DSQL difference: idle costs nothing.** There's no instance to keep
> warm. A cluster that isn't doing work bills only for its storage, which for
> an empty cluster is nothing.

We don't need it, so let's delete it. The console won't let me yet, because
deletion protection is on. Turn it off, then delete. That's exactly what it's
for.

<details>
<summary>Presenter notes</summary>

- Delete the **console** one (the older one), not the blog's: step 10 needs
  the blog. The blog's ID is the start of `$PGHOST`, in pane A's
  `create-cluster` output.
- If its deletion protection is off, skip the protection line or show where
  the setting lives.
- **Following along:** `aws dsql update-cluster --identifier <console-cluster-id> --no-deletion-protection-enabled`,
  then `aws dsql delete-cluster --identifier <console-cluster-id>`.
</details>

### Step 10 · Change data capture · 29:00–32:00

Every committed change, streamed to Kinesis. First, a stream for it. It's
on-demand, so there's nothing to size, with 10 MiB records, because CDC splits
large rows into fragments of up to 10 MiB:

```sh pane events
aws kinesis create-stream --stream-name dat401-cdc --stream-mode-details StreamMode=ON_DEMAND --max-record-size-in-ki-b 10240
```

It's active about five seconds later:

```sh pane events
aws kinesis describe-stream-summary --stream-name dat401-cdc
```
```text
{
    "StreamDescriptionSummary": {
        "StreamName": "dat401-cdc",
        "StreamARN": "arn:aws:kinesis:us-west-2:111122223333:stream/dat401-cdc",
        "StreamStatus": "ACTIVE",
        "StreamModeDetails": {
            "StreamMode": "ON_DEMAND"
        },
        ...
        "MaxRecordSizeInKiB": 10240
    }
}
```

<!-- stage: highlight StreamARN -->
Start the viewer on it. It reads from the latest position, so it shows only
new changes:

```sh pane events
./events/target/release/events --stream <kinesis-stream-arn> --table comments
```

Then, in the console: the blog's cluster → create a CDC stream → target the Kinesis
stream → create. Now add a comment, from psql:

```sql
SELECT id AS post_id FROM posts WHERE slug LIKE 'post-%' LIMIT 1 \gset
```
```sql
INSERT INTO comments (post_id, user_id, body) VALUES (:'post_id', :'user_id', 'Hello from the stage');
```

It shows up in the viewer straight away:

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
<summary>Following along: the CDC stream from the CLI</summary>

DSQL writes to the Kinesis stream through a service role that trusts `dsql.amazonaws.com`. The
console can create the role for you; the CLI steps are in
[Getting started with CDC streams](https://docs.aws.amazon.com/aurora-dsql/latest/userguide/cdc-setup.html).
Then:

```sh
aws dsql create-stream --cluster-identifier <cluster-id> \
  --target-definition kinesis={streamArn=<kinesis-stream-arn>,roleArn=<role-arn>} \
  --ordering UNORDERED --format JSON
```
</details>

<details>
<summary>Presenter notes</summary>

- `create-stream` fails if `dat401-cdc` already exists. Between rehearsals:
  `aws kinesis delete-stream --stream-name dat401-cdc`.
- The viewer's `--stream` takes the ARN, copied from the
  `describe-stream-summary` output (highlighted first).
</details>

### Step 11 · Multi-Region · 32:00–35:00

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
- one multi-Region DSQL cluster, peered between us-west-2 and us-east-1, with
  the same schema choices as the live build: UUID keys, comments keyed by
  `(post_id, comment_id)`, and covering indexes for the home and category pages.

<!-- beat -->

The page has one column per Region:
- **Round trip:** browser to that Region and back. From Las Vegas, us-west-2
  is closer.
- **In Aurora DSQL:** database time only, about the same in both Regions,
  because each reads from its local endpoint.

Open a post; with "Keep both columns on the same page" on, both columns follow.
Write a comment in the us-west-2 column, and watch us-east-1:

```
Posted in us-west-2, read here N ms after it was saved.
```

That's the next read in the other Region, not a replication delay we're
waiting out. In our measurements, a write spent about **80–190 ms** in the
database (the commit crosses the country) and a read about **11–14 ms**. For a
blog that's 90% reads, that's the right trade-off.

> **★ DSQL difference: no replication lag to code around.** The
> "read-your-writes in another Region" problem doesn't exist. There's no
> primary to fail over to, and no stale replica to route around.

<details>
<summary>Presenter notes</summary>

- `demo/preflight.sh` ~30 minutes before warms both APIs, so the first click
  isn't a cold start.
- One sentence on the architecture: one CloudFront distribution serves the page
  from S3 and routes `/api/<region>/*` to an IAM-authenticated Lambda function URL
  in each Region (only the distribution can call them).
</details>

<details>
<summary>Following along: deploy it yourself</summary>

`cd demo && ./deploy.sh` (about 10 minutes the first time; it's idempotent), then
`./preflight.sh`. Details in [`demo/README.md`](demo/README.md).
</details>

---

## 40:00–44:00 · Scale

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

## 44:00–45:00 · Wrap-up

What we did in 45 minutes, and what DSQL did differently:

| We did | DSQL made it different |
|---|---|
| Created a cluster | Seconds; nothing to size, patch or fail over |
| Connected with psql | IAM tokens, no passwords |
| Wrote a schema | Sequences need a cache, a round trip per ID with `CACHE 1`; UUIDs need nothing |
| Chose primary keys | The key is the physical layout: random to spread writes, grouped to read together |
| Loaded 1.1M rows | 3,000 rows per transaction; batch and parallelize |
| Read a query plan | The plan shows the DPU cost |
| Loaded a post's comments | A primary-key range scan: 0.5 ms, no index |
| Added an index | Built as a background job; covering indexes matter |
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
# Steps 1-11: your clusters (turn off deletion protection first if it's on)
aws dsql update-cluster --identifier <id> --no-deletion-protection-enabled
aws dsql delete-cluster --identifier <id>

# Step 10: the CDC stream, then the Kinesis stream
aws dsql delete-stream --cluster-identifier <cluster-id> --stream-identifier <stream-id>
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
- **An `Index Scan` where you expected `Index Only Scan`**: if the query selects
  the primary key, `INCLUDE` it in the index too. DSQL doesn't serve the key
  from a secondary index in an index-only scan otherwise.
