-- Steps 1-3, typed by hand on stage (cheat sheet: don't \i this).
-- A deliberately small schema with the same key design as the real one.
-- At the start of step 4 it's replaced by the real one:
--   DROP TABLE comments, posts;  \i sql/01-schema.sql   (the loader adds 02-first-rows.sql)

-- Step 1: posts, with a UUID primary key.
CREATE TABLE posts (
  id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  title text NOT NULL,
  created_at timestamptz DEFAULT now()
);

-- Said, not shown: "couldn't we use integers?" A sequence is a shared counter
-- in a distributed database, so an identity column needs a cache:
--   CACHE 1      dense and in order, but each id is a round trip (~7 ms; a
--                3,000-row INSERT that generates its ids took 20.6 s)
--   CACHE 65536  blocks of ids handed out without going back to the counter
--                (the same 3,000 rows: 117 ms), but expect gaps and jumps, and
--                ids don't follow commit order
-- Without a CACHE it's an error. (`serial` doesn't exist in DSQL, for the same
-- reason.)
-- A UUID needs no counter at all, which is why every table here uses them.

-- Comments: the key starts with post_id. The table is stored by its primary
-- key, so a post's comments sit together and "comments for a post" is one
-- range scan. No index on post_id needed.
--   Sequential vs random: random keys spread writes across the whole key space
--   (more of the fleet shares the load: DynamoDB's partition-key advice);
--   grouping by post_id makes reading a post's comments one contiguous read.
--   The price is a hotspot per post, and no blog needs comments on one post to
--   scale without limit.
CREATE TABLE comments (
  post_id uuid NOT NULL,
  comment_id uuid NOT NULL DEFAULT gen_random_uuid(),
  body text NOT NULL,
  created_at timestamptz DEFAULT now(),
  PRIMARY KEY (post_id, comment_id)
);

-- Step 2
INSERT INTO posts (title) VALUES ('Hello, DSQL'), ('Second post');
SELECT * FROM posts;
SELECT id AS post_id FROM posts WHERE title = 'Hello, DSQL' \gset
INSERT INTO comments (post_id, body) VALUES (:'post_id', 'First!');

-- Step 3: fast, because there's nothing here.
\timing on
SELECT title, created_at FROM posts ORDER BY created_at DESC LIMIT 20;

SELECT p.title, c.body, c.created_at
FROM posts p JOIN comments c ON c.post_id = p.id;

-- Step 4: the real schema (the one above is missing what a real app needs),
-- then sample data (05-load-sample-data.sh, which also adds 02-first-rows.sql).
DROP TABLE comments, posts;
\i sql/01-schema.sql
\d+ posts
\d+ users
