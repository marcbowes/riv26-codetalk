#!/usr/bin/env bash
# Step 5: fill cluster A with sample data, generated server-side with
# generate_series, so nothing big crosses the conference Wi-Fi.
#
#   PGHOST=<id>.dsql.us-west-2.on.aws ./sql/05-load-sample-data.sh
#
# Started as the script Kiro wrote in the first dry run (same approach), then
# tuned: one IAM token and one psql session per worker instead of per batch, a
# single metadata.category (step 9 indexes metadata->>'category'), and comments
# dated after their post.
#
# DSQL limits shape it: each INSERT is its own transaction of at most BATCH
# (3,000) rows and well under 10 MiB. Ids are deterministic, so rows can
# reference each other without lookups, and with ON CONFLICT DO NOTHING the
# script is safe to re-run:
#   users     md5 of a counter, as a uuid
#   posts     POST_ID_BASE + n, set explicitly (OVERRIDING SYSTEM VALUE). Letting
#             the CACHE 1 identity generate them would cost a round trip to the
#             shared counter per row (~20 s per 3,000). The sequence is moved
#             past them at the end with setval.
#   comments  (post_id, comment_id): the post comes from hashint8 of the
#             counter (1 to ~26 comments per post), comment_id is an md5 uuid
#
# Needs the real schema (sql/01-schema.sql). No foreign keys yet (step 7).
#
# Environment: PGHOST (required), AWS_REGION (default: from PGHOST),
#   USERS=1000 POSTS=100000 COMMENTS=1000000 BATCH=3000 PARALLEL=8
#   POST_ID_BASE=1000 (loaded posts get ids above it; hand-made ones stay below)
set -euo pipefail

: "${PGHOST:?set PGHOST to the cluster endpoint, e.g. abc123.dsql.us-west-2.on.aws}"
REGION="${AWS_REGION:-$(cut -d. -f3 <<<"$PGHOST")}"
USERS="${USERS:-1000}"
POSTS="${POSTS:-100000}"
COMMENTS="${COMMENTS:-1000000}"
BATCH="${BATCH:-3000}"
PARALLEL="${PARALLEL:-8}"
POST_ID_BASE="${POST_ID_BASE:-1000}"
((BATCH <= 3000)) || { echo "BATCH must be <= 3000 (DSQL's per-transaction row limit)" >&2; exit 1; }

export PGHOST PGUSER="${PGUSER:-admin}" PGDATABASE="${PGDATABASE:-postgres}" PGSSLMODE=require
# One token for the whole run: it only has to be valid when each worker connects.
PGPASSWORD="$(aws dsql generate-db-connect-admin-auth-token --hostname "$PGHOST" --region "$REGION" --expires-in 3600)"
export PGPASSWORD

# Posts are spread evenly over the two years up to 2 hours ago (post n is
# older than post n+1, plus up to an hour of jitter); a comment lands between
# an hour after its post and now.
NOW_EPOCH="$(date +%s)"
SPAN=$((730 * 86400))
START_EPOCH=$((NOW_EPOCH - SPAN))
POST_SPAN=$((SPAN - 7200))

CATEGORIES="ARRAY['databases','distributed-systems','serverless','performance','security','devops','frontend','career']"
TAGS="ARRAY['dsql','postgres','lambda','occ','indexes','json','multi-region','iam','drizzle','typescript','rust','latency','cost','testing','observability']"
FIRST="ARRAY['Alex','Sam','Priya','Jordan','Mei','Diego','Fatima','Noah','Aisha','Lukas','Yuki','Omar','Elena','Kofi','Hana','Mateo','Zoe','Ravi','Ingrid','Tariq','Lena','Chen','Amara','Felix']"
LAST="ARRAY['Rivera','Nakamura','Okafor','Schmidt','Patel','Haddad','Kowalski','Tanaka','Byrne','Silva','Novak','Ahmed','Larsen','Moreau','Kim','Osei','Fischer','Reyes','Lindqvist','Mensah','Rossi','Ivanova','Costa','Walsh']"
OPENERS="ARRAY['A practical guide to','Notes on','Why we moved to','Debugging','Lessons from','Rethinking','Ten mistakes with','Scaling','Testing','What I learned about']"
CLOSERS="ARRAY['',' in production',' at scale',' the hard way',' on a budget',', revisited',' in TypeScript',' across Regions']"
TOPICS="ARRAY['optimistic concurrency','connection pooling','multi-Region writes','async indexes','foreign keys','JSONB columns','Lambda cold starts','IAM auth tokens','query plans','identity columns','change data capture','retries','idempotency keys','time zones','Drizzle ORM']"
SENTENCES="ARRAY['We started with the simplest thing that could work.','The numbers surprised us.','It turned out the bottleneck was somewhere else entirely.','EXPLAIN ANALYZE told us what the dashboards could not.','The fix was smaller than the investigation.','Retries hid the problem until load doubled.','Nobody had touched that code in two years.','A small experiment beat a long design review.','The second attempt was much simpler.','We wrote down what we would do differently next time.']"
REPLIES="ARRAY['Great post!','Thanks for sharing.','Could you say more about the retries?','We hit exactly this last month.','How did you measure that?','This saved me an afternoon.','Interesting, I would have guessed the opposite.','Bookmarked.','Did you try a partial index?','What did the query plan look like before?']"

users_sql() { # lo hi
  cat <<SQL
INSERT INTO users (id, username, display_name, created_at)
SELECT md5('user:' || g)::uuid,
       lower(($FIRST)[1 + g % 24]) || '_' || g,
       ($FIRST)[1 + g % 24] || ' ' || ($LAST)[1 + (g / 24) % 24],
       to_timestamp($START_EPOCH + random() * $SPAN)
FROM generate_series($1, $2) g
ON CONFLICT DO NOTHING;
SQL
}

posts_sql() { # lo hi
  cat <<SQL
INSERT INTO posts (id, author_id, slug, title, body, metadata, created_at)
OVERRIDING SYSTEM VALUE
SELECT $POST_ID_BASE + g,
       md5('user:' || (1 + floor(random() * $USERS)::int))::uuid,
       'post-' || g,
       t.title,
       array_to_string(ARRAY(
         SELECT ($SENTENCES)[1 + floor(random() * 10)::int]
         FROM generate_series(1, 4 + g % 8)
       ), ' '),
       jsonb_build_object(
         'category', ($CATEGORIES)[1 + floor(random() * 8)::int],
         'tags', jsonb_build_array(($TAGS)[1 + floor(random() * 15)::int], ($TAGS)[1 + floor(random() * 15)::int]),
         'readingMinutes', 2 + g % 13,
         'seo', jsonb_build_object('description', t.title || ': what we learned.')
       ),
       to_timestamp($START_EPOCH + g::float8 * $POST_SPAN / $POSTS + random() * 3600)
FROM generate_series($1, $2) g,
     -- "+ 0 * g" ties the subquery to the row, so random() runs per post, not once.
     LATERAL (SELECT ($OPENERS)[1 + floor(random() * 10)::int + 0 * g] || ' ' || ($TOPICS)[1 + floor(random() * 15)::int]
                     || ($CLOSERS)[1 + floor(random() * 8)::int] AS title) t
ON CONFLICT DO NOTHING;
SQL
}

comments_sql() { # lo hi
  cat <<SQL
INSERT INTO comments (post_id, comment_id, user_id, body, created_at)
SELECT $POST_ID_BASE + c.n,
       md5('comment:' || g)::uuid,
       md5('user:' || (1 + floor(random() * $USERS)::int))::uuid,
       ($REPLIES)[1 + floor(random() * 10)::int],
       to_timestamp(c.post_epoch + 3600 + random() * ($NOW_EPOCH - c.post_epoch - 3600))
FROM generate_series($1, $2) g,
     LATERAL (SELECT n, $START_EPOCH + n::float8 * $POST_SPAN / $POSTS AS post_epoch
              FROM (SELECT 1 + (abs(hashint8(g::bigint)) % $POSTS)::int AS n) r) c
ON CONFLICT DO NOTHING;
SQL
}

# retrying <sql>: run a batch, retrying it (in the same session) if it fails.
# Batches are idempotent (ON CONFLICT DO NOTHING), so a retry is always safe.
# This matters right after DDL: the first statements against freshly created
# tables can fail with OC001 (schema has been updated by another transaction).
# The last attempt runs with ON_ERROR_STOP, so a batch that keeps failing ends
# the worker with an error.
retrying() {
  printf '%s\n' "$1" '\if :ERROR' '\! sleep 1' "$1" '\if :ERROR' '\! sleep 3' \
    '\set ON_ERROR_STOP 1' "$1" '\set ON_ERROR_STOP 0' '\endif' '\endif'
}

# worker <sql fn> <lo> <hi> <errfile>: one psql session runs every batch in
# [lo, hi]. It exits non-zero only if a batch fails all three attempts
# (pipefail makes that psql's exit status); errors go to <errfile>.
worker() {
  local fn=$1 lo=$2 hi=$3 errfile=$4 a b
  for ((a = lo; a <= hi; a += BATCH)); do
    b=$((a + BATCH - 1))
    ((b > hi)) && b=$hi
    retrying "$("$fn" "$a" "$b")"
  done | psql -X -q -v ON_ERROR_STOP=0 >/dev/null 2>"$errfile"
}

# load <label> <total> <sql fn>: split [1, total] across PARALLEL workers.
load() {
  local label=$1 total=$2 fn=$3 per lo hi started i=0 failed=0 retried
  local pids=() errs=()
  started=$SECONDS
  per=$(((total + PARALLEL - 1) / PARALLEL))
  for ((lo = 1; lo <= total; lo += per)); do
    hi=$((lo + per - 1))
    ((hi > total)) && hi=$total
    errs+=("$TMP/$label.$i.err")
    worker "$fn" "$lo" "$hi" "${errs[$i]}" &
    pids+=("$!")
    i=$((i + 1))
  done
  for i in "${!pids[@]}"; do
    if ! wait "${pids[$i]}"; then
      failed=1
      echo "  $label: a batch failed three times:" >&2
      grep ERROR "${errs[$i]}" | tail -1 >&2
    fi
  done
  retried=$(cat "${errs[@]}" | grep -c ERROR || true)
  printf '  %-9s %9d rows in %3ds' "$label" "$total" $((SECONDS - started))
  ((retried > 0)) && printf '  (%d batch attempts retried, e.g. %s)' "$retried" \
    "$(grep -h -o -m1 'OC00[01]\|[A-Za-z ]*exceeded' "${errs[@]}" | head -1)"
  printf '\n'
  return "$failed"
}

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

echo "Loading into $PGHOST ($PARALLEL workers, $BATCH rows per transaction)"
total_started=$SECONDS
load users "$USERS" users_sql
load posts "$POSTS" posts_sql
load comments "$COMMENTS" comments_sql
# Point the posts identity past the loaded ids, so new posts don't collide.
psql -X -q -c "SELECT setval(pg_get_serial_sequence('posts', 'id'), (SELECT max(id) FROM posts))" >/dev/null
psql -X -q -c "ANALYZE users" -c "ANALYZE posts" -c "ANALYZE comments"
echo "Done in $((SECONDS - total_started))s"
psql -X -c "SELECT (SELECT count(*) FROM users) AS users, (SELECT count(*) FROM posts) AS posts, (SELECT count(*) FROM comments) AS comments"
