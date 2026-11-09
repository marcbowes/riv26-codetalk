-- Step 5: add the missing index. CREATE INDEX ASYNC returns a job_id
-- immediately and builds in the background without blocking writes.
-- Measured on the step-4 data (100k posts, 1M comments), October 2026.

-- No index for "comments for a post": comments' primary key starts with
-- post_id, so that's already a primary-key range scan (step 4's EXPLAIN:
-- ~0.5 ms, ~0.02 DPU). Keyed by comment id alone it was a full scan
-- (~1,030 ms, ~191 DPU), and needed a (post_id, created_at) index to get to
-- ~1.7 ms.

-- Recent posts: a covering index, for
--   SELECT title, created_at FROM posts ORDER BY created_at DESC LIMIT 20;
-- In DSQL the table *is* its primary-key B-tree (\d+ posts: "PRIMARY KEY ...
-- INCLUDE (every column)", "Access method: btree_table"), so an index on
-- created_at alone would need a lookup per row to fetch title. (The talk used
-- to show that index, and the planner keeping the full scan + sort; it's out.)
CREATE INDEX ASYNC posts_recent_idx ON posts (created_at) INCLUDE (title);
--   before: full primary-key scan + top-N sort, ~208 ms, ~121 DPU
--   after:  Index Only Scan Backward, ~0.7 ms, ~0.04 DPU (no sort, no lookups)
-- If the query also selects the primary key (id), INCLUDE that too: DSQL doesn't
-- serve the key from a secondary index in an index-only scan otherwise.
-- (The build on 100k posts: ~25 s.)

-- Watch the jobs (DSQL also runs its own ANALYZE jobs, hence the filter)
SELECT job_id, status, object_name, details FROM sys.jobs
WHERE job_type = 'INDEX_BUILD' ORDER BY start_time DESC;

-- Block until one finishes (returns succeeded = t/f):
--   CALL sys.wait_for_job('<job_id>');
--
-- A failed build leaves an INVALID index behind: DROP INDEX it and retry.
-- No planner knobs to reach for instead: SET enable_sort etc. are "not supported".
-- Then re-run 05-explain.sql.
