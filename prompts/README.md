# AI prompts for the live build

Each prompt is deliberately plain. The point on stage is how little
DSQL-specific prompting is needed. The DSQL-specific knowledge comes from the
DSQL agent skill / MCP server, installed once beforehand (Kiro power, or the
Skills CLI for Claude Code). Install it before rehearsing, and say so on stage.

Rules for every AI step (from OUTLINE.md):

1. High success rate. Rehearse each prompt until it reliably works.
2. Not too long. Each task gets a time budget, below.
3. Room to teach. Each one has a "while it runs" topic.

After a good rehearsal, save the output under `prompts/fallback/<step>/` so a
bad run on stage can be swapped for a known-good one.

| Step | Prompt | Budget | While it runs |
|---|---|---|---|
| 1 | [01-create-cluster.md](01-create-cluster.md) | < 1 min | Clusters create in seconds; the console cluster is already up |
| 4 | [04-api.md](04-api.md) | shown, not run (~1 min) | What the agent produced: the DSQL Drizzle adapter, `transactionWithRetry` on writes. In rehearsal it took ~11 min, too long to wait for on stage |
| 5 | (pre-built: `sql/05-load-sample-data.sh`) | ~15 s | The per-transaction limits (3,000 rows / 10 MiB), and why the script batches. [05-sample-data.md](05-sample-data.md) is the prompt that produced the first version |

Placeholders: `$CLUSTER_ENDPOINT` is cluster A's endpoint, e.g.
`abc123.dsql.us-west-2.on.aws`.

Only step 1's prompt runs live now. The step-5 loader generates its data
server-side, so a slow conference uplink doesn't matter.
