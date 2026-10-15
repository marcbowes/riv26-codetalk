# Step 5: sample data

**Not run live any more.** In the first dry run Kiro took 8m13s to write
and run this. Its script, tuned, is now `sql/05-load-sample-data.sh`, which
takes about 15 s. This prompt is kept as the reference for how it started.
If it's ever used again, spell out the shape: Kiro wrote
`metadata.categories` as an array, and step 9 needs a single
`metadata.category`.


```
Fill my blog database with realistic sample data: 1,000 users, 100,000 posts
and about 1,000,000 comments. Give each post metadata with a category (pick
from 8), a few tags and readingMinutes. Spread created_at over the last two
years. It should finish in under two minutes.
```

Expected: batches under the 3,000-rows-per-transaction limit, run
concurrently. If the agent generates rows server-side with `generate_series`,
it doesn't depend on the uplink.

Tune the sizes in rehearsal. They need to be big enough that step 5's EXPLAIN
shows clearly bad plans, and small enough to stay inside the budget.
