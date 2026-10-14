-- Step 5: after the sample data has loaded. VERBOSE adds the DPU cost
-- estimate at the bottom of the plan.

-- Pick a post that has comments
SELECT post_id FROM comments LIMIT 1 \gset

EXPLAIN ANALYZE VERBOSE
SELECT p.id, p.title, u.display_name AS author, p.created_at
FROM posts p
JOIN users u ON u.id = p.author_id
ORDER BY p.created_at DESC
LIMIT 20;

EXPLAIN ANALYZE VERBOSE
SELECT c.body, u.username, c.created_at
FROM comments c
JOIN users u ON u.id = c.user_id
WHERE c.post_id = :'post_id'
ORDER BY c.created_at;
