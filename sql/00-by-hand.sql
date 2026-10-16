-- Steps 1-3, typed by hand on stage (cheat sheet: don't \i this).
-- A deliberately small schema with the same key design as the real one.
-- Before step 4 it's thrown away for the real one:
--   DROP TABLE comments, posts;  \i sql/01-schema.sql  \i sql/02-first-rows.sql

-- Step 1: posts with an integer identity id. Type it the "normal Postgres"
-- way first:
CREATE TABLE posts (
  id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  title text NOT NULL,
  created_at timestamptz DEFAULT now()
);
--   ERROR:  identity column is not supported without an explicit cache size.
--           please define CACHE greater than or equal to 65536 or equal to 1
--
-- Discuss: in a distributed database a sequence is a shared counter.
--   CACHE 1      every id comes from the shared counter: dense and in order,
--                but each one is a round trip (~7 ms; a 3,000-row INSERT that
--                generates its ids took 20.6 s)
--   CACHE 65536  each connection reserves a block: no coordination per insert
--                (the same 3,000 rows: 117 ms), but gaps, and ids don't follow
--                commit order
-- Posts are written rarely, so CACHE 1 (`serial` doesn't exist in DSQL, for
-- the same reason):
CREATE TABLE posts (
  id bigint GENERATED ALWAYS AS IDENTITY (CACHE 1) PRIMARY KEY,
  title text NOT NULL,
  created_at timestamptz DEFAULT now()
);

-- Comments are the high-write table: random UUIDs, no counter at all. And the
-- key starts with post_id. The table is stored by its primary key, so a post's
-- comments sit together and "comments for a post" is one range scan. No index
-- on post_id needed.
--   Sequential vs random: random keys spread writes across the whole key space
--   (more of the fleet shares the load: DynamoDB's partition-key advice);
--   clustering by post_id makes reading a post's comments one contiguous read.
--   The price is a hotspot per post, and no blog needs comments on one post to
--   scale without limit.
CREATE TABLE comments (
  post_id bigint NOT NULL,
  comment_id uuid NOT NULL DEFAULT gen_random_uuid(),
  body text NOT NULL,
  created_at timestamptz DEFAULT now(),
  PRIMARY KEY (post_id, comment_id)
);

-- Step 2 (pane A)
INSERT INTO posts (title) VALUES ('Hello, DSQL') RETURNING id;      -- 1
INSERT INTO posts (title) VALUES ('Second post') RETURNING id;      -- 2
INSERT INTO comments (post_id, body) VALUES (1, 'First!');

-- Step 2 (pane B, a different connection): still dense, because every id
-- came from the shared counter.
INSERT INTO posts (title) VALUES ('From pane B') RETURNING id;      -- 3
-- (With CACHE 65536, this would have been 65537: pane B's connection would
-- have reserved its own block.)

-- Step 3
SELECT id, title, created_at FROM posts ORDER BY created_at DESC LIMIT 10;

SELECT p.title, c.body, c.created_at
FROM posts p JOIN comments c ON c.post_id = p.id;

-- Then the real schema, before kicking off steps 4 and 5.
DROP TABLE comments, posts;
\i sql/01-schema.sql
\i sql/02-first-rows.sql
