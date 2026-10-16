-- Steps 1-3, typed by hand on stage (cheat sheet: don't \i this).
-- A deliberately small schema with the same key design as the real one.
-- At the end of step 3 it's thrown away for the real one:
--   DROP TABLE comments, posts;  \i sql/01-schema.sql  \i sql/02-first-rows.sql

-- Step 1: posts, with a UUID primary key.
CREATE TABLE posts (
  id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  title text NOT NULL,
  created_at timestamptz DEFAULT now()
);

-- Aside: "couldn't we use integers?" Try it on a throwaway table:
CREATE TABLE ids (id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY, note text);
--   ERROR:  identity column is not supported without an explicit cache size.
--           please define CACHE greater than or equal to 65536 or equal to 1
--
-- In a distributed database a sequence is a shared counter, so you choose:
--   CACHE 1      every id comes from the shared counter: dense and in order,
--                but each one is a round trip (~7 ms; a 3,000-row INSERT that
--                generates its ids took 20.6 s)
--   CACHE 65536  blocks of ids are reserved and handed out without going back
--                to the shared counter (the same 3,000 rows: 117 ms), but expect
--                gaps and jumps (in rehearsal one session jumped to 65537), and
--                ids don't follow commit order
-- (`serial` doesn't exist in DSQL, for the same reason.)
CREATE TABLE ids (id bigint GENERATED ALWAYS AS IDENTITY (CACHE 65536) PRIMARY KEY, note text);
DROP TABLE ids;
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
INSERT INTO posts (title) VALUES ('Hello, DSQL'), ('Second post') RETURNING id;
INSERT INTO comments (post_id, body) SELECT id, 'First!' FROM posts WHERE title = 'Hello, DSQL';

-- Step 3
SELECT id, title, created_at FROM posts ORDER BY created_at DESC LIMIT 10;

SELECT p.title, c.body, c.created_at
FROM posts p JOIN comments c ON c.post_id = p.id;

-- Then the real schema, before step 5 loads sample data into it.
DROP TABLE comments, posts;
\i sql/01-schema.sql
\i sql/02-first-rows.sql
