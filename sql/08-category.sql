-- Step 8: query posts by category from the JSONB front matter.

-- Slow: full scan
EXPLAIN ANALYZE VERBOSE
SELECT id, title, created_at
FROM posts
WHERE metadata->>'category' = 'databases'
ORDER BY created_at DESC
LIMIT 20;

-- Expression index: the expression must match the query exactly
CREATE INDEX ASYNC posts_category_idx ON posts ((metadata->>'category'), created_at);
-- CALL sys.wait_for_job('<job_id>');

-- Fast: index scan (re-run the EXPLAIN above)

-- The trap: `->` returns jsonb, not text, so this doesn't match the index
EXPLAIN ANALYZE VERBOSE
SELECT id, title, created_at
FROM posts
WHERE metadata->'category' = '"databases"'
ORDER BY created_at DESC
LIMIT 20;
