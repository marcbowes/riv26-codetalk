# Step 7: the foreign key race

Two psql sessions side by side, both connected to cluster A. Setup in either:

```sql
SELECT id AS post_id FROM posts WHERE slug = 'hello-dsql' \gset
SELECT id AS user_id FROM users WHERE username = 'raluca' \gset
```

(`\gset` variables are per session: run both lines in **both** panes.)

## 1. Without a foreign key: both commit

| Session A | Session B |
|---|---|
| `BEGIN;` | |
| `DELETE FROM posts WHERE id = :'post_id';` | |
| | `BEGIN;` |
| | `INSERT INTO comments (post_id, user_id, body) VALUES (:'post_id', :'user_id', 'Nice post!');` |
| `COMMIT;` → succeeds | |
| | `COMMIT;` → **succeeds** |

They wrote different rows, so OCC sees no conflict. Now there are orphans: B's
comment, plus the step-2 "First!" comment, since nothing cascaded. Look them up
by `post_id`: that uses step 6's `comments_post_idx`, whereas an anti-join over
a million comments would be a slow full scan on stage.

```sql
SELECT id, body FROM comments WHERE post_id = :'post_id';   -- 2 rows
```

## 2. Add the FK: validation fails on the orphan

On a table that already has rows, the FK has to be added `NOT VALID` and then
validated as a job:

```sql
ALTER TABLE comments
  ADD CONSTRAINT comments_post_fk FOREIGN KEY (post_id)
  REFERENCES posts (id) ON DELETE CASCADE NOT VALID;

ALTER TABLE ASYNC comments VALIDATE CONSTRAINT comments_post_fk;  -- returns job_id
CALL sys.wait_for_job('<job_id>');                                -- succeeded = f
SELECT status, details FROM sys.jobs WHERE job_id = '<job_id>';
--  failed | insert or update on table "comments" violates foreign key constraint "comments_post_fk"
```

The `NOT VALID` constraint is already enforced for new writes (an insert with
a bogus `post_id` fails with 23503). Validation is about the rows that are
already there.

## 3. Clean up, validate again

```sql
DELETE FROM comments WHERE post_id = :'post_id';

ALTER TABLE ASYNC comments VALIDATE CONSTRAINT comments_post_fk;
CALL sys.wait_for_job('<job_id>');                                -- succeeded = t
```

## 4. Re-run the race: B fails at commit

Re-insert the post first (not `02-first-rows.sql`: its users already exist):

```sql
INSERT INTO posts (author_id, slug, title, body)
SELECT id, 'hello-dsql', 'Hello, DSQL', 'again' FROM users WHERE username = 'marc';
```

Then
**reconnect pane B (`\c`)** and `\gset` again in both panes.

Why reconnect: pane B's session still has the schema from before the
`ALTER TABLE`. Its first statement touching `comments` fails with
`schema has been updated by another transaction (OC001)` instead of reaching
the race. That's another thing you could teach (OC001 is a schema conflict,
OC000 a data conflict), but it muddies this demo.

| Session A | Session B |
|---|---|
| `BEGIN;` | |
| `DELETE FROM posts WHERE id = :'post_id';` | |
| | `BEGIN;` |
| | `INSERT INTO comments (post_id, user_id, body) VALUES (:'post_id', :'user_id', 'Nice post!');` → succeeds (B's snapshot still has the post) |
| `COMMIT;` → succeeds | |
| | `COMMIT;` → `ERROR: change conflicts with another transaction (OC000) (SQLSTATE 40001)` |

Teaching point (from the DSQL FK docs): checks read the transaction's
start-time snapshot rather than taking locks. At commit, DSQL treats the
referenced row as if it were read `FOR KEY SHARE`, so a concurrent delete of
the parent is a write-write conflict and one side loses. No locks while the
transaction runs, and still no orphans.

Also worth adding `users` FKs (no orphans there, so validation passes first time):

```sql
ALTER TABLE comments ADD CONSTRAINT comments_user_fk
  FOREIGN KEY (user_id) REFERENCES users (id) NOT VALID;
ALTER TABLE ASYNC comments VALIDATE CONSTRAINT comments_user_fk;

ALTER TABLE posts ADD CONSTRAINT posts_author_fk
  FOREIGN KEY (author_id) REFERENCES users (id) NOT VALID;
ALTER TABLE ASYNC posts VALIDATE CONSTRAINT posts_author_fk;
```
