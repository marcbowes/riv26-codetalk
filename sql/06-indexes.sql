-- Step 6: add the missing indexes. Each CREATE INDEX ASYNC returns a job_id
-- immediately and builds in the background without blocking writes.
-- Measured on the step-5 data (100k posts, 1M comments), October 2026.

-- Comments for a post. created_at is in the key so the rows come back
-- already sorted (the Sort node disappears too).
CREATE INDEX ASYNC comments_post_idx ON comments (post_id, created_at);
--   before: Full Scan, 999,991 rows removed by filter, ~1,030 ms, ~191 DPU
--   after:  Index Scan, ~1.7 ms, ~0.04 DPU            (build: ~45 s on 1M rows)

-- Recent posts: the obvious index...
CREATE INDEX ASYNC posts_created_at_idx ON posts (created_at);
-- ...and the planner ignores it for
--   SELECT title, created_at FROM posts ORDER BY created_at DESC LIMIT 20;
-- Still a full primary-key scan + top-N sort (~216 ms, ~121 DPU). Why: in DSQL
-- the table *is* its primary-key B-tree (\d+ posts: "PRIMARY KEY ... INCLUDE
-- (every column)", "Access method: btree_table"), so a secondary-index hit
-- needs a second lookup to fetch title. Make the index cover the query:
CREATE INDEX ASYNC posts_recent_idx ON posts (created_at) INCLUDE (title);
--   after:  Index Only Scan Backward, ~0.9 ms, ~0.04 DPU (no sort, no lookups)
DROP INDEX posts_created_at_idx;

-- Watch the jobs (DSQL also runs its own ANALYZE jobs, hence the filter)
SELECT job_id, status, object_name, details FROM sys.jobs
WHERE job_type = 'INDEX_BUILD' ORDER BY start_time DESC;

-- Block until one finishes (returns succeeded = t/f):
--   CALL sys.wait_for_job('<job_id>');
--
-- A failed build leaves an INVALID index behind: DROP INDEX it and retry.
-- No planner knobs to reach for instead: SET enable_sort etc. are "not supported".
-- Then re-run 05-explain.sql.
