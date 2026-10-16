-- Step 5: after the sample data has loaded. VERBOSE adds the DPU cost
-- estimate at the bottom of the plan.

-- Recent posts (the home page): full scan + top-N sort (~208 ms, ~121 DPU)
EXPLAIN ANALYZE VERBOSE
SELECT title, created_at FROM posts ORDER BY created_at DESC LIMIT 20;

-- Pick a post that has comments
SELECT post_id FROM comments LIMIT 1 \gset

-- Comments for a post (the post page): already a primary-key range scan,
-- because comments' key starts with post_id (~0.5 ms, ~0.02 DPU)
EXPLAIN ANALYZE VERBOSE
SELECT body, created_at FROM comments WHERE post_id = :'post_id' ORDER BY created_at;
