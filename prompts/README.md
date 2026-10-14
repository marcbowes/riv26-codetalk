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
| 4 | [04-api.md](04-api.md) | ~8 min, in the background | Steps 5–7 happen meanwhile |
| 5 | [05-sample-data.md](05-sample-data.md) | < 2 min | The per-transaction limits (3,000 rows / 10 MiB), and why the script batches |
| 8 | [08-html.md](08-html.md) | ~3 min, in the background | Step 9 happens meanwhile |

Placeholders: `$CLUSTER_ENDPOINT` is cluster A's endpoint, e.g.
`abc123.dsql.us-east-1.on.aws`.

Network: steps 4 and 5 talk to the cluster from wherever the agent runs. On
conference Wi-Fi, prefer a dev box in the cluster's Region. Also, step 5 should
generate data server-side or keep rows small, so a slow uplink doesn't blow
the time budget.
