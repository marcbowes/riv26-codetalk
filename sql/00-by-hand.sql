-- Steps 1-3, typed by hand on stage (cheat sheet: don't \i this).
-- A deliberately small schema. Before step 4 it's thrown away for the real
-- one:  DROP TABLE comments, posts;  \i sql/01-schema.sql

-- Step 1
CREATE TABLE posts (
  id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  title text NOT NULL,
  created_at timestamptz DEFAULT now()
);

CREATE TABLE comments (
  id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  post_id uuid NOT NULL,
  body text NOT NULL,
  created_at timestamptz DEFAULT now()
);

-- Optional detour: integer ids. `serial` doesn't exist in DSQL, and an
-- identity column needs an explicit cache, which makes the error a good
-- teaching moment:
--   CREATE TABLE t (id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY);
--   ERROR:  identity column is not supported without an explicit cache size.
--           please define CACHE greater than or equal to 65536 or equal to 1
--   CREATE TABLE t (id bigint GENERATED ALWAYS AS IDENTITY (CACHE 65536) PRIMARY KEY);

-- Step 2
INSERT INTO posts (title) VALUES ('Hello, DSQL') RETURNING id;
INSERT INTO comments (post_id, body) SELECT id, 'First!' FROM posts;

-- Step 3
SELECT title, created_at FROM posts ORDER BY created_at DESC LIMIT 10;

SELECT p.title, c.body, c.created_at
FROM posts p JOIN comments c ON c.post_id = p.id;

-- Then the real schema, before kicking off steps 4 and 5
DROP TABLE comments, posts;
\i sql/01-schema.sql
\i sql/02-first-rows.sql
