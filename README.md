# Amazon Aurora DSQL: A developer's perspective (DAT401, re:Invent 2026)

Code and scripts for the code talk. [OUTLINE.md](OUTLINE.md) has the run of show.

| Path | What | Used in |
|---|---|---|
| [`sql/`](sql) | psql scripts: schema, queries, EXPLAIN, async indexes, the foreign key race, the JSONB expression index | Live build, steps 1–9 |
| [`prompts/`](prompts) | The AI prompts, with time budgets and fallbacks | Live build, steps 1, 4, 5, 8 |
| [`events/`](events) | Rust CLI that tails DSQL CDC records from Kinesis and shows per-stage lag | Step 11 and the finale |
| [`demo/`](demo) | The polished app: two-Region DSQL cluster, Lambda API per Region, React UI on CloudFront, CDK, `deploy.sh` | Minutes 35–45 |
| [`demo/loadgen/`](demo/loadgen) | Rust load generator with a live TPS display | Minutes 40–45 |

## Quick start

```sh
cd demo && ./deploy.sh                          # idempotent; writes demo/outputs.json
cd events && cargo run --release                # tail the step-11 CDC stream
cd demo/loadgen && cargo run --release -- --rps 300 --ramp 20 --duration 2m
```

Each folder's README has the details.
