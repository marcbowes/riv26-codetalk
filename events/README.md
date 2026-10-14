# events

Tails the Kinesis data stream that Aurora DSQL CDC writes to, and shows each change as it
arrives with its lag broken down by stage. Used in step 11: add a comment in the page, watch it
arrive.

```
lag per stage: cdc = commit→CDC · kinesis = CDC→Kinesis · here = Kinesis→here · total = commit→here
commit       op     table    tx           cdc kinesis    here   total  row
17:05:08.069 INSERT comments ea2akesm   102ms    53ms   225ms   380ms  "Great post!" by user ee0a7cf3… on post d9bd532b…
17:05:13.712 UPDATE posts    fjktjwbv   142ms    49ms   120ms   311ms  "Building a blog on Aurora DSQL, part 1" by 6380fa53… [databases] #dsql #postgres #serverless · 7 min · body 269 B
17:05:16.010 DELETE comments 4jwsa4bk   149ms    50ms   194ms   393ms  id=4a00ab9d…
17:05:18.710 INSERT posts    ztbds37u   178ms    50ms   100ms   328ms  [3 fragments] "The long read: CDC end to end" by ee0a7cf3… [streaming] #cdc #kinesis · 14 min · body 2.7 KB
● 4 shards · behind 0ms · 1.4 ev/s · total p50 342ms p99 464ms (last 60s, n=9) · 1 chunked
```

## Build

```sh
cd events
cargo build --release        # binary: target/release/events
cargo test
```

AWS credentials and Region come from the usual chain (env vars, `AWS_PROFILE`, SSO, …).
Reading needs `kinesis:DescribeStreamSummary`, `kinesis:ListShards`, `kinesis:GetShardIterator`
and `kinesis:GetRecords` on the stream, plus `kms:Decrypt` if the stream uses a customer
managed key.

## Step 11: live demo

The Kinesis stream, CDC service role and CDC stream are all set up by hand (see the DSQL
["Getting started with CDC streams"](https://docs.aws.amazon.com/aurora-dsql/latest/userguide/cdc-setup.html)
guide: `ON_DEMAND`, `MaxRecordSizeInKiB` 10240). Point this tool at the Kinesis stream:

```sh
cd events
./target/release/events --stream <name|arn> --region us-east-1
./target/release/events --stream <name|arn> --table comments   # just the comments from the page
```

- Start it **before** creating the CDC stream in the console. It starts at `latest`, so it
  shows only new changes. The first records turn up once the CDC stream is `ACTIVE`.
- Add a comment in the web page. One green `INSERT comments` line appears with the comment
  text and four lag numbers.
- If you missed something, `--from 5m` re-reads the last five minutes (AT_TIMESTAMP).
  `--from trim-horizon` reads everything retained.
- A pinned status line at the bottom refreshes every ~2 s: open shards, the highest
  `MillisBehindLatest`, events/s, and total-lag p50/p99 over the last 60 s.
- On Ctrl-C it prints a one-line summary.

## Under load

```sh
./target/release/events stats --stream <name|arn>
```

`stats` redraws once a second:
- events/s by table and op (last second, 10 s average, total);
- p50/p99/max for each lag stage over the last 10 s;
- Kinesis shards, max `MillisBehindLatest`, GetRecords/s, records/s, throttles and errors;
- chunked-record reassembly counters;
- the cluster ID(s) seen in `source.cluster`.

On exit it prints the final frame. When stdout isn't a terminal it prints one summary line per
second instead.

Reading the numbers: a rising **commit→CDC** is DSQL's CDC falling behind (the
`BehindSourceLag` metric). A rising **CDC→Kinesis** is delivery into Kinesis (on-demand
scaling, `KINESIS_THROUGHPUT_EXCEEDED`). A rising **Kinesis→here** together with a rising
`MillisBehindLatest` means this reader is the bottleneck, not DSQL.

## Rehearsing without AWS

```sh
./target/release/events replay samples/blog.jsonl              # recorded pacing, gaps capped at 2 s
./target/release/events replay samples/blog.jsonl --speed 0    # all at once
./target/release/events replay samples/blog.jsonl --stats
```

Replay runs the same pipeline (parsing, reassembly, dedup, lag) as live mode. Every lag stays as
recorded. The commit time shown is shifted so the lines look live.

`samples/blog.jsonl` holds the blog schema:
- two `users` inserts;
- a post, then an update to it;
- three comments, one of which is deleted;
- a chunked post: its `after` image split into 3 fragments, read out of order from different
  shards, arriving before and after the main record.

The sample is small. Real CDC only chunks records over 9 MiB, but the envelope matches what
DSQL emits.

To rehearse with real data, record a live session and replay it later:

```sh
./target/release/events --save /tmp/rehearsal.jsonl      # also works with stats
./target/release/events replay /tmp/rehearsal.jsonl
```

### Replay file format

One JSON object per line. Each line is either a bare CDC record (it has a top-level `type`) or a
wrapper:

```json
{"arrival": "2026-10-14T17:05:08.296Z", "received": "2026-10-14T17:05:08.521418489Z",
 "shard": "shardId-000000000001", "data": {"type": "full", "op": "c", "...": "..."}}
```

- `arrival` is Kinesis `ApproximateArrivalTimestamp`; `received` is when the reader got the
  record. Both are optional. They can be RFC 3339 strings or epoch numbers in s, ms, µs or ns
  (chosen by magnitude). Without them, the stages that need them show `—`.
- `data` may also be the record's JSON text as a string.
- AWS CLI records (`Data` in base64, `ApproximateArrivalTimestamp`) work as is, so you can do
  `aws kinesis get-records … | jq -c '.Records[]' > f.jsonl`.

## Lag stages

| column | stage | computed as |
|---|---|---|
| `cdc` | commit → CDC processed | `ts_ns − source.ts_ns` |
| `kinesis` | CDC → arrived in Kinesis | `ApproximateArrivalTimestamp − ts_ns` |
| `here` | Kinesis → shown here ¹ | local clock when processed − `ApproximateArrivalTimestamp` |
| `total` | commit → shown here ¹ | local clock when processed − `source.ts_ns` |

The nanosecond fields are used when present (`ts_ns`, `source.ts_ns`), falling back to `*_ms`.
`ApproximateArrivalTimestamp` only has millisecond precision, so `kinesis` can read slightly
negative (e.g. `-0.4ms`). For a chunked record, arrival and "here" are taken from the last piece
to arrive, because the change isn't usable until every piece is in. Colors: green under 1 s,
yellow under 5 s, red beyond.

¹ `here` and `total` compare AWS timestamps with this machine's clock, so they're only as
accurate as local clock sync. Check with `sntp time.aws.com` on macOS or `chronyc tracking` on
Linux before the talk. A skewed clock shifts both columns by the same amount. The `cdc` and
`kinesis` columns only use AWS-side timestamps and don't depend on the local clock.

## How it reads the stream

- **All shards.** CDC uses random partition keys, so changes and the fragments of a chunked
  record can be on any shard. Shards are listed at start and re-listed every `--relist` (10 s),
  and immediately when a shard closes. On-demand streams reshard as load changes. New shards
  are read from TRIM_HORIZON. A shard's poller stops once the shard is closed and fully read.
  Shards that were already closed at startup are skipped with `--from latest`.
- **Polling.** One GetRecords per shard every `--poll-ms` (250 ms, under the 5 reads/s/shard
  limit). On throttling, exponential backoff with jitter (counted as "throttled"). An expired or
  invalid iterator is re-acquired after the last sequence number read. Other errors are shown
  at most once per 10 s, and polling keeps going.
- **Backpressure.** If the display can't keep up, the pollers wait. The backlog then shows as
  `MillisBehindLatest` and Kinesis→here instead of growing memory.

## Record handling

- `type` `full`, `chunked` and `fragment` are handled.
  - Fragments are buffered by `chunk_id` and can arrive before the main record, after it, or on
    another shard.
  - Once all `total_fragments` are present, they're sorted by `index` and concatenated.
  - The CRC32C of the text (a decimal string) is checked, then the text is parsed as the image.
  - On a CRC mismatch or invalid JSON, a warning is printed and the record is dropped.
  - Partial records older than `--chunk-ttl` (2 min) are dropped with a warning. So are
    fragments whose main record never arrives, which is normal right after starting at `latest`.
- Duplicates (at-least-once delivery) are dropped and counted. A full record is a duplicate if
  it has the same txId, table, commit time and row image as one already seen. For chunked
  records, a repeated `chunk_id` is a duplicate.
- Unknown fields are ignored. An unknown `op` is shown as `X?`; an unknown `type` is skipped.
  Both produce a single warning. A `source.version` with a major version other than 1 also
  produces a single warning, and those records are still shown.
- Value formatting:
  - `*_at` numeric columns (µs since the epoch) are shown as local times.
  - json/jsonb columns, which arrive as strings containing JSON, are shown as compact JSON.
  - uuids are shortened to 8 hex digits, and long text is truncated.
  - Deletes show the primary key from `before`.
- `--json` prints the records themselves (reassembled images inline) instead of summary lines.
  Status and warnings go to stderr, so `events --json | jq` works.
