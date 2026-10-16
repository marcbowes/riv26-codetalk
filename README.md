# Amazon Aurora DSQL: A developer's perspective (DAT401, re:Invent 2026)

Code and scripts for the code talk.

- **[TALK.md](TALK.md)**: the full talk, written out. Follow along from here if you couldn't attend.
- [OUTLINE.md](OUTLINE.md): planning notes and run of show.

| Path | What | Used in |
|---|---|---|
| [`speaker/`](speaker) | Terminal teleprompter that plays TALK.md beat by beat: what to say, what to run (`c` copies it), key points, ahead/behind schedule | On stage |
| [`sql/`](sql) | psql scripts: schema, queries, EXPLAIN, async indexes, the foreign key race, the JSONB expression index | Live build, steps 1–9 |
| [`prompts/`](prompts) | The AI prompts, with time budgets and fallbacks | Live build, step 1 (step 4 shows the API prompt) |
| [`events/`](events) | Rust CLI that tails DSQL CDC records from Kinesis and shows per-stage lag | Step 10 |
| [`demo/`](demo) | The polished app: two-Region DSQL cluster, Lambda API per Region, React UI on CloudFront, CDK, `deploy.sh` | Minutes 35–45 |
| [`demo/loadgen/`](demo/loadgen) | Rust load generator with a live TPS display | Minutes 40–45 |

## Quick start

```sh
cd demo && ./deploy.sh                          # idempotent; writes demo/outputs.json
cd events && cargo run --release -- --stream <kinesis stream>   # step 10
cd demo/loadgen && cargo run --release -- --rps 300 --ramp 20 --duration 2m
```

Each folder's README has the details.
