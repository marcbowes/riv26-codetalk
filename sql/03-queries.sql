-- Step 3: the queries the blog needs. These become the API (step 4) and the
-- EXPLAIN targets (step 5).

-- Recent posts, with author
SELECT p.id, p.title, u.display_name AS author, p.created_at
FROM posts p
JOIN users u ON u.id = p.author_id
ORDER BY p.created_at DESC
LIMIT 20;

-- Comments for a post, with usernames
SELECT c.body, u.username, c.created_at
FROM comments c
JOIN users u ON u.id = c.user_id
WHERE c.post_id = (SELECT id FROM posts WHERE slug = 'hello-dsql')
ORDER BY c.created_at;
