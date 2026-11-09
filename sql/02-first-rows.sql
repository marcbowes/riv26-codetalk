-- The first rows: two users, a post, a comment. 05-load-sample-data.sh adds
-- them (step 4) before the bulk data; later steps use them (hello-dsql, raluca).

INSERT INTO users (username, display_name)
VALUES ('marc', 'Marc Bowes'), ('raluca', 'Raluca Constantin')
RETURNING id, username;

INSERT INTO posts (author_id, slug, title, body, metadata)
SELECT id, 'hello-dsql', 'Hello, DSQL',
       'Serverless scale, relational model. Let''s build a blog.',
       '{"category": "databases", "tags": ["dsql", "postgres"], "readingMinutes": 3}'
FROM users WHERE username = 'marc'
RETURNING id, slug;

INSERT INTO comments (post_id, user_id, body)
SELECT p.id, u.id, 'First!'
FROM posts p, users u
WHERE p.slug = 'hello-dsql' AND u.username = 'raluca'
RETURNING post_id, comment_id, created_at;
