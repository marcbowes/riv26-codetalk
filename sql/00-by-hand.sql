-- Steps 1-3, typed by hand on stage (cheat sheet: don't \i this).
-- A deliberately small schema. Before step 4 it's thrown away for the real
-- one:  DROP TABLE comments, posts;  \i sql/01-schema.sql

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
-- Discuss: in a distributed database a sequence is a shared counter, so
--   CACHE 65536  each connection reserves a block of ids: no coordination per
--                insert, but gaps, and ids don't follow commit order
--   CACHE 1      every id comes from the shared sequence: dense, but every
--                insert has to go to that sequence
-- (`serial` doesn't exist in DSQL, for the same reason.)

CREATE TABLE posts (
  id bigint GENERATED ALWAYS AS IDENTITY (CACHE 65536) PRIMARY KEY,
  title text NOT NULL,
  created_at timestamptz DEFAULT now()
);

-- Comments are the high-write table: uuid ids need no sequence at all.
CREATE TABLE comments (
  id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  post_id bigint NOT NULL,
  body text NOT NULL,
  created_at timestamptz DEFAULT now()
);

-- Step 2 (pane A)
INSERT INTO posts (title) VALUES ('Hello, DSQL') RETURNING id;      -- 1
INSERT INTO posts (title) VALUES ('Second post') RETURNING id;      -- 2
INSERT INTO comments (post_id, body) VALUES (1, 'First!');

-- Step 2 (pane B, a different connection): the cache made visible
INSERT INTO posts (title) VALUES ('From pane B') RETURNING id;      -- 65537

-- Step 3
SELECT id, title, created_at FROM posts ORDER BY created_at DESC LIMIT 10;

SELECT p.title, c.body, c.created_at
FROM posts p JOIN comments c ON c.post_id = p.id;

-- Then the real schema, before kicking off steps 4 and 5. It uses uuid ids
-- throughout (the AI-generated API and sample data build on it).
DROP TABLE comments, posts;
\i sql/01-schema.sql
\i sql/02-first-rows.sql
