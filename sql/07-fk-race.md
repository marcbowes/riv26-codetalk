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

They wrote different rows, so OCC sees no conflict. Now there's an orphan:

```sql
SELECT c.id, c.body
FROM comments c LEFT JOIN posts p ON p.id = c.post_id
WHERE p.id IS NULL;
```

## 2. Add the FK: validation fails on the orphan

On a table that already has rows, the FK has to be added `NOT VALID` and then
validated as a job:

```sql
ALTER TABLE comments
  ADD CONSTRAINT comments_post_fk FOREIGN KEY (post_id)
  REFERENCES posts (id) ON DELETE CASCADE NOT VALID;

ALTER TABLE ASYNC comments VALIDATE CONSTRAINT comments_post_fk;  -- returns job_id
CALL sys.wait_for_job('<job_id>');                                -- false
SELECT status, details FROM sys.jobs WHERE job_id = '<job_id>';  -- failed
```

With PostgreSQL semantics, a `NOT VALID` constraint is already enforced for new
writes (confirm on DSQL). Validation is about the rows that are already there.

## 3. Clean up, validate again

```sql
DELETE FROM comments c
WHERE NOT EXISTS (SELECT 1 FROM posts p WHERE p.id = c.post_id);

ALTER TABLE ASYNC comments VALIDATE CONSTRAINT comments_post_fk;
CALL sys.wait_for_job('<job_id>');                                -- true
```

## 4. Re-run the race: B fails at commit

Re-insert a post first (`\i sql/02-first-rows.sql` or a fresh slug), then
`\gset` again in both panes.

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
