-- Step 3 (end): the real schema, loaded with \i after the hand-typed one.
--
-- No foreign keys and no secondary indexes yet: those are steps 6, 7 and 9.
-- `metadata` is created now, so step 5's sample data can fill it. Adding it
-- later would mean an UPDATE across every post, and DSQL caps a transaction
-- at 3,000 modified rows.
--
-- Key choices (the step-1 discussion):
--   posts.id     sequential: an identity column with CACHE 1. Posts are written
--                rarely, so a round trip to the shared counter per post (~7 ms)
--                is fine, and ids stay dense and in order.
--   comments     PRIMARY KEY (post_id, comment_id). The table is stored by its
--                primary key, so a post's comments sit together: "comments for
--                a post" is one range scan of the primary key, no secondary
--                index. comment_id is a random UUID: no counter to share. The
--                cost is a per-post hotspot, which a blog never needs to scale.

CREATE TABLE users (
  id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  username text NOT NULL UNIQUE,
  display_name text NOT NULL,
  created_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE posts (
  id bigint GENERATED ALWAYS AS IDENTITY (CACHE 1) PRIMARY KEY,
  author_id uuid NOT NULL,
  slug text NOT NULL UNIQUE,
  title text NOT NULL,
  body text NOT NULL,
  metadata jsonb NOT NULL DEFAULT '{}'::jsonb,
  created_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE comments (
  post_id bigint NOT NULL,
  comment_id uuid NOT NULL DEFAULT gen_random_uuid(),
  user_id uuid NOT NULL,
  body text NOT NULL,
  created_at timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (post_id, comment_id)
);
