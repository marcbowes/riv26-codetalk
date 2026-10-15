# Step 4: API, CLI and server (background)

```
In ./blog, build a TypeScript API for my blog on Aurora DSQL using Drizzle ORM.
The cluster endpoint is $CLUSTER_ENDPOINT; connect as admin. The tables already
exist: read the schema from the database.

Operations: ListPosts (newest first, with author), GetPost (with its comments
and each commenter's username), CreatePost, AddComment.

Give me an HTTP server and a small CLI that calls it over HTTP. Start the server
and use the CLI to check that every operation works.
```

Expected:
- Uses `@aws/aurora-dsql-drizzle`, and therefore IAM auth with no passwords.
- Uses `transactionWithRetry` for writes.
- The server is on localhost:3000.

Must finish before step 7 starts deleting rows.

First dry run (Kiro): about 11 minutes. It first used plain `drizzle-orm` with
the connector and had to be nudged to the adapter; its CLI called the service
layer directly, hence "calls it over HTTP".
