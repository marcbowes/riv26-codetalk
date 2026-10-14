# Storyboard: re:Invent 2026 Aurora DSQL code talk

Slot: **60 min, with audience questions taken throughout.** We plan **~45 min of content**, which leaves **~15 min for questions** as they come up.

| Minutes | Part |
|---|---|
| 0–5 | Intro: what DSQL is, the agenda |
| 5–35 | Build it: 12 steps, live |
| 35–40 | The polished app on Lambda, in two Regions |
| 40–45 | Finale: load, and the stack scales out on its own |

## Concept
**"Build a discussion forum, then watch it scale with no intervention."**

- **Domain:** a discussion forum with this basic shape:
  - **Posts**, with a title, body, author, and (from step 9) JSONB metadata such as a category and tags.
  - **Comments** on each post, shown with the username and ordered by time.
  - **Users**, who write posts and comments.

  The schema is simple, and reads must be strongly consistent: you see your own comment immediately, in every Region.

- **How we build (minutes 5–35):** three tools, each used where it's strongest.
  - **Web console:** where the visual matters (creating clusters, CloudWatch, PITR, CDC, multi-Region setup).
  - **`psql`:** where explaining specific results matters (queries, `EXPLAIN ANALYZE`, the FK race).
  - **AI prompting** (Claude Code or Kiro, with the DSQL plugin/power): where the point is writing code, and showing how little DSQL-specific prompting is needed.
- **Rules for high-latency tasks** (AI tasks, index builds, the restore). Each one must:
  1. have a high success rate,
  2. not take too long,
  3. give us something to explain or teach while it runs.

  Each step below notes its expected duration, what we teach while it runs, and its fallback.
- **Pre-built before the talk:** only what minutes 35–45 need.
  - **The polished app** on **AWS Lambda Web Functions** (launched 2026-10-12): an Express/Hono server listening on a port, with no adapters.
    - What it gives us:
        - **A URL in each Region** (a `PerRegion` endpoint): we deploy once, Lambda replicates the code to us-east-1 and us-east-2, and each Region gets its own HTTPS domain (`….lambda-web.<region>.on.aws`).
        - **Scales to zero.**
        - **One execution environment serves many concurrent requests.**
        - **Billed on active CPU**, so time spent waiting on the database is nearly free (memory still accrues).
        - Regions at launch include us-east-1, us-east-2 and us-west-2, so they line up with the DSQL cluster.
        - Docs: https://docs.aws.amazon.com/lambda/latest/dg/web-functions.html
  - **A multi-Region DSQL cluster** behind it (us-east-1 + us-east-2, with a witness in us-west-2).
  - **The load generator fleets** (one per Region) for the finale.
  - **The CDC pieces:** a Kinesis stream and an IAM role for step 11, plus the pre-built CLI tool that streams events.
  - The single-Region clusters used in minutes 5–35 are created **live**.

## Domain schema
```sql
-- step 1
CREATE TABLE users    (id UUID PRIMARY KEY DEFAULT gen_random_uuid(), username TEXT NOT NULL, created_at TIMESTAMPTZ DEFAULT now());
CREATE TABLE posts    (id UUID PRIMARY KEY DEFAULT gen_random_uuid(), author_id UUID NOT NULL,
                       title TEXT NOT NULL, body TEXT, created_at TIMESTAMPTZ DEFAULT now());
CREATE TABLE comments (id UUID PRIMARY KEY DEFAULT gen_random_uuid(), post_id UUID NOT NULL, author_id UUID NOT NULL,
                       body TEXT NOT NULL, created_at TIMESTAMPTZ DEFAULT now());
-- step 7
ALTER TABLE comments ADD CONSTRAINT comments_post_fk FOREIGN KEY (post_id) REFERENCES posts (id) NOT VALID;
-- step 9
ALTER TABLE posts ADD COLUMN metadata JSONB;   -- {"category":"announcements","tags":["dsql","postgres"]}
```

## Scene overview

| # | Clock | Min | Step | Tool | Feature(s) |
|---|---|---|---|---|---|
| — | 0:00 | 5 | Intro: DSQL = DynamoDB's serverless scale + PostgreSQL's relational model. Agenda. | Slides | |
| 1 | 0:05 | 4 | Create a cluster in the console, create a **second** one with AI, connect with `psql`, create the schema | Console, AI, `psql` | Fast cluster creation, IAM auth, UUID keys |
| 2 | 0:09 | 2 | Insert a post, add a comment | `psql` | |
| 3 | 0:11 | 2 | Queries: recent posts, comments on a post with usernames | `psql` | |
| 4 | 0:13 | 1 | **AI task A (background):** a Drizzle API (ListPosts, GetPost, …) with a CLI and a server. The agent starts the server and validates it with the CLI. | AI | Drizzle adapter |
| 5 | 0:14 | 3 | **AI task B (alongside A):** generate sample data. Then `EXPLAIN ANALYZE` for plans and cost. | AI, `psql` | `EXPLAIN ANALYZE VERBOSE`, DPU estimate |
| 6 | 0:17 | 2 | Add the missing indexes, for example on `created_at` to sort posts | `psql` | `CREATE INDEX ASYNC` |
| 7 | 0:19 | 4 | Foreign keys: a side-by-side race where a post is deleted while a comment is added. Add the FK and validate it. | `psql` ×2 | FK, OCC (40001), `NOT VALID` + validate |
| 8 | 0:23 | 3 | Task A is done, so show the API working. **AI task C (background):** an HTML page that uses the API, running locally. | AI | |
| 9 | 0:26 | 3 | Add JSONB `metadata`, query by category, add an expression index | `psql` | JSONB, expression index |
| 10 | 0:29 | 3 | The second cluster costs nothing (CloudWatch). Clean it up… delete the **wrong** one. "Oops." Recover with PITR. | Console | Scale to zero, PITR |
| 11 | 0:32 | 2 | Set up a CDC stream, and show a pre-built CLI tool streaming the events | Console, CLI | CDC → Kinesis |
| 12 | 0:34 | 1 | Multi-Region, explained while setting up a multi-Region cluster in the console | Console | Multi-Region active-active |
| — | 0:35 | 5 | The polished app on Lambda with a web UI, in two Regions | Browser | Web Functions, multi-Region |
| — | 0:40 | 5 | Finale: generate load. Lambda + DSQL scale out automatically, with no intervention. | Browser, CloudWatch | Scale |

---

## Scenes

### Intro (0:00, 5 min). Slides.
- Title slide (RIV26 speaker template), then Marc and Raluca.
- **What DSQL is:** it blends the **serverless scale of DynamoDB** with the **rich relational model of PostgreSQL**. No instances to size, high availability, active-active across Regions, and PostgreSQL SQL, transactions and drivers.
- The agenda: what we'll build in the next 30 minutes, then the polished version running in two Regions, then the load test.
- Out loud: **questions are welcome throughout**.
- Line: "Last year we built a banking app. This year we're building a discussion forum, and we'll finish with it running at scale in two Regions."

### Step 1 · Two clusters, connect, schema (0:05, 4 min). Console + AI + `psql`.
- **Console:** create a single-Region cluster. Cluster creation takes seconds. While it finishes, point out what you didn't have to choose: instance size, storage, replicas.
- **AI, while the console one finishes:** *"Create a second Aurora DSQL cluster in us-east-1 called `scratch`."* It's a short, high-success task, and it shows the agent using the DSQL tools. Leave **deletion protection off** on both clusters (this sets up step 10).
- **`psql`:** connect to the first cluster with an IAM token (one `aws dsql generate-db-connect-admin-auth-token` line). Point out: no passwords, tokens expire, and you connect to the cluster endpoint directly.
- Create `users`, `posts` and `comments`. Say in one sentence that the UUID keys spread writes out.
- **Fallback:** if the AI cluster creation stalls, create the second cluster in the console.

### Step 2 · Insert a post, add a comment (0:09, 2 min). `psql`.
- Insert a user and a post, then a comment on it. It's plain PostgreSQL SQL.

### Step 3 · Queries (0:11, 2 min). `psql`.
- Recent posts: `SELECT … FROM posts ORDER BY created_at DESC LIMIT 20;`
- Comments for a post, with usernames:
  ```sql
  SELECT c.body, u.username, c.created_at
  FROM comments c JOIN users u ON u.id = c.author_id
  WHERE c.post_id = $1 ORDER BY c.created_at;
  ```
- These are the queries steps 5–6 make fast.

### Step 4 · AI task A, the API (0:13, 1 min to start, then runs in the background until step 8).
- Prompt: *"Using Drizzle ORM with the Aurora DSQL adapter, build an API for this schema with ListPosts, GetPost (with its comments and usernames), CreatePost and CreateComment. Generate a server and a CLI that calls it. Start the server and use the CLI to validate every operation."*
- Out loud: "We won't wait for this. We'll come back to it." What it teaches: the prompt doesn't spell out any DSQL specifics, because the plugin/power supplies them (IAM tokens, OCC retry).
- **Duration target:** it must finish by step 8 (about 10 minutes). Rehearse until it reliably does.
- **Fallback:** a known-good branch with the same API, checked out in step 8.

### Step 5 · AI task B, sample data, then `EXPLAIN ANALYZE` (0:14, 3 min). AI + `psql`.
- In a second agent window, alongside task A: *"Generate sample data: 1,000 users, 100,000 posts and 1,000,000 comments, with a few popular posts getting most of the comments."* Settle the counts at rehearsal, so the task finishes within ~2 minutes and the tables are big enough for the indexes to matter.
- While it runs, explain what DSQL meters: compute, reads and writes in DPUs.
- Then:
  ```sql
  EXPLAIN ANALYZE VERBOSE <the comments-for-a-post query from step 3>;
  ```
  It shows a full scan plus a sort, and the **Statement DPU Estimate**, which is the cost of this one query.
- **Fallback:** a pre-written `generate_series` seed script.

### Step 6 · Add the missing indexes (0:17, 2 min). `psql`.
- `CREATE INDEX ASYNC posts_created_idx ON posts (created_at);` for sorting recent posts.
- `CREATE INDEX ASYNC comments_post_time_idx ON comments (post_id, created_at);` for a post's comments.
- `SELECT sys.wait_for_job(...)`, then re-run `EXPLAIN ANALYZE VERBOSE`. The plan becomes an index scan with no sort, and both latency and DPU drop.
- What it teaches: index builds are always asynchronous in DSQL, so they never block writes.

### Step 7 · Foreign keys: the race (0:19, 4 min). Two `psql` sessions.
- **On screen (one shared screen, Marc's laptop):** a tmux split with two `psql` sessions, each with its own colour and a big name in the prompt (for example `\set PROMPT1 '%[%033[1;32m%]marc%[%033[0m%]> '` and a red `raluca>`). Marc types in both, and Raluca voices the second session. Pre-type the statements into each pane's history, so it's just ↑ and Enter.
- **Without an FK:**
  ```sql
  -- marc>                                  -- raluca>
  BEGIN;                                     BEGIN;
  DELETE FROM posts WHERE id = $1;           INSERT INTO comments (post_id, …) VALUES ($1, …);
  COMMIT;  -- ok                             COMMIT;  -- ok → an orphaned comment
  ```
- **Add the FK** as an online schema change: `ALTER TABLE comments ADD CONSTRAINT comments_post_fk FOREIGN KEY (post_id) REFERENCES posts (id) NOT VALID;`. Then validate the existing rows asynchronously. **VERIFY** that `ALTER TABLE ASYNC … VALIDATE CONSTRAINT` works for FKs.
- **Repeat the race:** this time the second `COMMIT` fails with `ERROR 40001 OC000`.
- What it teaches: optimistic concurrency. There are no locks and no waiting, and the conflict is detected at commit time through an implicit `KEY SHARE` on the referenced post. The loser retries, and the Drizzle adapter's opt-in OCC retry does that for the app.
- **Fallback:** the orphan-cleanup `DELETE` is pre-written, in case validation finds orphans from the race.

### Step 8 · The API works, then AI task C, the HTML page (0:23, 3 min). AI.
- Back to task A's window. Show the CLI validating ListPosts, GetPost, CreatePost and CreateComment against the server, then call it once more live.
- Start task C: *"Build a simple HTML page that uses this API: a list of recent posts, a post page with its comments, and a comment box. Run it locally."*
- What it teaches while it runs: the API is ordinary Drizzle code, so the same server could run anywhere, including Lambda (which leads into minute 35).
- **Duration target:** a couple of minutes. Show it at the end of step 9.
- **Fallback:** a known-good page on the same branch as task A's fallback.

### Step 9 · JSONB metadata and an expression index (0:26, 3 min). `psql`.
- `ALTER TABLE posts ADD COLUMN metadata JSONB;`, then fill in a category and tags for the sample posts. Mention in one line that **large JSON/JSONB/text values are compressed by default**.
- Query by category: `SELECT … FROM posts WHERE metadata->>'category' = 'announcements' ORDER BY created_at DESC LIMIT 20;`
- `EXPLAIN` shows a full scan. Then add the **expression index**: `CREATE INDEX ASYNC posts_category_idx ON posts ((metadata->>'category'), created_at);`, re-run, and it's an index scan.
- Then flip to task C's page, running locally.

### Step 10 · Scale to zero, then "oops", then PITR (0:29, 3 min). Console.
- "We want to show CDC next. But first, notice we created **two** clusters in step 1."
- Open the second (`scratch`) cluster's CloudWatch metrics: DPU usage is at zero, because an idle cluster isn't costing us anything beyond storage. That's the scale-to-zero point.
- "Let's clean it up anyway." Then delete the **wrong** cluster: the main one, with our schema and data. Pretend to be alarmed.
- Introduce **PITR** and restore the main cluster in the console to a point just before the delete.
- What it teaches while the restore runs: what PITR covers and how far back it goes. Also note the restored cluster gets a new endpoint, and that deletion protection would have prevented this in real life.
- **VERIFY** the restore time. It must be short enough for this scene. If it's longer, start the restore and move on to step 11 on the pre-built multi-Region cluster, coming back to the restored one later.

### Step 11 · CDC stream (0:32, 2 min). Console + CLI.
- In the console, create a CDC stream to the pre-created Kinesis stream (with the pre-created IAM role).
- Run the **pre-built CLI tool** that tails the stream. Add a comment in `psql` and the event appears.
- One-line teaching beats:
  - Delivery is at-least-once.
  - **Write-set compaction:** an insert followed by a delete in one transaction emits nothing.
  - It's an outbox pattern without an outbox table.
- **Fallback:** a recorded run of the CLI tool.

### Step 12 · Multi-Region (0:34, 1 min). Console.
- Explain multi-Region active-active **verbally** while walking through creating a multi-Region cluster in the console: two peered Regions plus a witness Region, both Regions writable, strongly consistent.
- There's no need to wait for it, because the pre-built app in the next scene already runs on one.

### The polished app on Lambda (0:35, 5 min). Browser.
- The app we just built, in polished form, running on **Lambda Web Functions** with a web UI. Show the Express code briefly: an ordinary `app.listen(port)`, with no Lambda handler.
- **On screen:** two browser tabs side by side, the **us-east-1 URL** and the **us-east-2 URL** of the same Web Function. Each page footer shows which Region served it.
- Each Region's server connects to **its local endpoint** of the multi-Region DSQL cluster, picked from a Region → endpoint map using `AWS_REGION`.
- Demo: post a comment in the east-1 tab, refresh the east-2 tab, and it's there immediately. Then do it the other way round.
- One line each:
  - A single CDC stream sees writes from both Regions.
  - The FK and OCC guarantees from step 7 hold across Regions.
  - If you want one global URL instead, Web Functions also offers a `MultiRegion` endpoint with latency-based routing (no health-based failover).
- **VERIFY:** that `AWS_REGION` is set inside a Web Function's environment.

### Finale: load, and automatic scale-out (0:40, 5 min). Browser + CloudWatch.
- Start the load generator across the Web Function's **5 PerRegion endpoints**. The fleet in each Region drives that Region's 5 URLs at 10K RPS each, so ~100K TPS across 2 Regions.
- **On screen:** the web UI staying responsive, next to CloudWatch showing Web Function execution environments and DSQL transactions climbing.
- Emphasis: **nobody touches anything**. Lambda adds execution environments and DSQL absorbs the writes, with no capacity planning, no provisioning and no intervention.
- Wrap slide: everything we used (the console, `psql`, AI with the DSQL plugin/power, Drizzle, async and expression indexes, FKs and OCC, JSONB, PITR, CDC, multi-Region, Lambda Web Functions), and thanks.

---

## Pre-talk prep checklist
- [ ] **Apply for tokens:** AWS promotional credits for the ~100K TPS load, and AI tool tokens for the stage machines. (Owner: Raluca, researching how.)
- [ ] Deployed before the talk:
  - the multi-Region DSQL cluster for the polished app
  - the Web Function with 5 `PerRegion` endpoints, each with a URL in us-east-1 and us-east-2. Endpoint 1 serves the browser + the load, and 2–5 serve the load only.
  - the `npm run deploy` script: create a revision, then move every endpoint's traffic to it (`PerRegion` endpoints don't auto-deploy)
  - the Kinesis stream, the IAM role for CDC, and the CLI tool that tails the stream
  - the CloudWatch dashboard for the finale
  - the load generator fleets, one in each Region
- [ ] Fallback branches for AI tasks A and C, and a seed script for task B.
- [ ] Rehearse each AI task for **success rate and duration**: A must finish by step 8, B within ~2 min, and C within ~2–3 min.
- [ ] Fallback recordings for steps 10 and 11, and for the finale.
- [ ] Verify:
  - FK `NOT VALID` + async validate
  - the PITR restore flow and time in the console
  - the CDC stream setup in the console, and how JSONB arrives in CDC records
  - `AWS_REGION` inside a Web Function (to pick the local DSQL endpoint), and `maxEnvironments` × pool sizing
  - the DSQL connection quotas under a Web Function scale-up
- [ ] **Check the demo account's Web Functions quotas** in us-east-1 and us-east-2: `GetWebAccountSettings` (it needs an AWS CLI new enough to have the `lambda-web` commands), or the Service Quotas console. If they're below 100K RPS / 2,000 vCPU, request increases for **requests per second per account** and **Arm64 vCPUs**. Also run a warm-up load in the weeks before, since new-account quotas rise with usage. The per-endpoint limit of 10K RPS per Region can't be raised, which is why there are 5 endpoints.
