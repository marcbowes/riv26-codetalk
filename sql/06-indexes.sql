-- Step 6: add the missing indexes. Each returns a job_id immediately.

CREATE INDEX ASYNC posts_created_at_idx ON posts (created_at);
CREATE INDEX ASYNC comments_post_idx ON comments (post_id, created_at);

-- Watch the jobs
SELECT job_id, status, object_name, details FROM sys.jobs ORDER BY start_time DESC;

-- Block until one finishes:
--   CALL sys.wait_for_job('<job_id>');
--
-- A failed build leaves an INVALID index behind: DROP INDEX it and retry.
-- Then re-run 05-explain.sql: index scans, lower DPU estimate.
