# loadgen

HTTP load generator for the finale of DAT401: "Lambda and DSQL scale out
automatically with no intervention. Look at the TPS."

It reads each Region's API URL from `demo/outputs.json`, collects real user
and post ids from the API, then drives a read-heavy blog mix against both
Regions at once. The full-screen display shows one big TPS number, a 60-second
sparkline, and a per-Region table.

```
 Aurora DSQL · multi-Region blog                                    RAMPING · 23 s to target   01:23 / 05:00

    ██    ██████      ██████  ██  ██  ██████  TPS  successful API calls per second, all Regions
  ████        ██          ██  ██  ██  ██           every call reads or writes Aurora DSQL
    ██    ██████        ████  ██████  ██████
    ██    ██      ██      ██      ██      ██  target 20,000/s   sending 12,400/s   peak 12,400 TPS
  ██████  ██████  ██  ██████      ██  ██████  in flight 210   posts 3,412   users 200

╭ TPS · last 60 s ─────────────────────────────────────────────────────────────────────── peak 12,400 ╮
│                                                                    ▁▁▁▁▂▂▂▂▃▃▃▃▄▄▄▄▅▅▅▅▆▆▆▆▇▇▇▇█████│
│                                   ▁▁▁▁▂▂▂▂▃▃▃▃▄▄▄▄▅▅▅▅▆▆▆▆▇▇▇▇██████████████████████████████████████│
╰─────────────────────────────────────────────────────────────────────────────────────────────────────╯
╭ Regions ─────────────────────────────────── latency of 2xx over the last second; errors since start ╮
│Region       TPS      rate  in-flight  p50 ms  p90 ms  p99 ms  DB p50  DB p99  4xx  429  5xx  timeout  connect│
│us-east-1  6,172   6,200/s        210      24      31      80     3.1     9.0    0    3    1        0        0│
│us-east-2  6,172   6,200/s        210      24      31      80     3.1     9.0    0    3    1        0        0│
│all       12,344  12,400/s        420      24      31      80     3.1     9.0    0    6    2        0        0│
╰─────────────────────────────────────────────────────────────────────────────────────────────────────╯
╭ Events ─────────────────────────────────────────────────────────────────────────────────────────────╮
│00:00 ramping to 20,000 req/s over 60 s                                                               │
╰─────────────────────────────────────────────────────────────────────────────────────────────────────╯
 q quit   +/- target ±25%   mix 90/10 read/write   HTTP/1.1   2 Region(s)
```

- **TPS** is successful (2xx) responses per second, over a sliding one-second
  window that updates four times a second.
- **rate** is what the generator is sending: it ramps to the target, and
  turns yellow while it is backing off after throttling.
- **in-flight** counts requests sent but not yet answered. It is an upper
  bound on that Region's Lambda concurrency.
- **p50/p90/p99** are end-to-end latencies of successful requests over the
  last second. **DB p50/p99** is the server-reported `timing.dbMs`, which
  shows that DB time is a small fraction of the total. These columns appear
  once responses carry it.
- **Error columns** are counts since the start. A count is bright when it
  grew in the last second.
- **Keys:** `q`/`Esc`/`Ctrl-C` stop. `+`/`-` (or Up/Down) move the target up
  or down 25%, re-ramped at the same slope as `--ramp`.

When stdout isn't a terminal (or with `--plain`), it prints one line per
second instead:

```
[ 00:05] TPS   2,001 | rate 2,000/s of 2,000/s | us-east-1 999/s in-flight 9 p50/p90/p99 8.8/13/15 ms db 2.3/5.8 ms err 4xx=0 429=0 5xx=0 timeout=0 connect=0 | us-east-2 1,001/s in-flight 8 ...
```

On exit it prints a summary: per-Region requests, average and peak TPS,
latency percentiles, errors by class and skipped sends, then a per-operation
breakdown. `--report run.json` also writes the config, totals, per-Region and
per-operation stats, the event log, and a per-second timeline, which is
useful for slides.

## Quick start

```sh
cd demo/loadgen
cargo build --release
./target/release/loadgen --rps 200 --ramp 20 --duration 2m   # reads ../outputs.json
```

The binary's full `--help` explains every flag. The important ones:

| Flag | Default | |
|---|---|---|
| `--outputs PATH` | `../outputs.json` | Reads `regions[].apiUrl` and `regions[].region`. If `../outputs.json` isn't found, it also looks next to this crate. |
| `--url URL` (repeatable) | | Use instead of `--outputs`. Region names come from `GET /health`. |
| `--region-weights` | even | `us-east-1=3,us-east-2=1`, or `3,1` in target order. |
| `--rps N` | 100 | Open loop: total req/s across Regions, sent on schedule whatever the latency. |
| `--concurrency N` | | Closed loop instead: N workers, each sending its next request when the last returns. |
| `--ramp SECS` | 60 | Linear ramp from zero. Gentle on purpose (see below). |
| `--duration` | until Ctrl-C | `90s`, `10m`, `1h30m`. |
| `--mix R/W` | `90/10` | Plus `--list-share` (40), `--category-share` (50), `--new-post-share` (0). |
| `--connections N` | 2048 | Max connections, which equals max in-flight requests, per Region. |
| `--timeout`, `--connect-timeout` | 10 s, 5 s | |
| `--threads N` | CPUs | Tokio worker threads. |
| `--plain`, `--report FILE`, `--title TEXT` | | |

## What it sends

**Bootstrap.**
1. `GET /health` on every target, which also reports the negotiated HTTP version.
2. From the first healthy Region: `GET /users?limit=200` and `GET /categories`.
3. `GET /posts?limit=100`, paged with `?before=<createdAt of the last post>`
   until `--bootstrap-posts` ids (3,000 by default).
4. Two pages per category, to reach older posts.

Paging stops early when a page adds nothing new. Posts inserted in one
transaction share a `created_at`, and paging by timestamp skips the rest of
such a batch; seed data with spread-out timestamps pages fully.

**Mix** (defaults; percentages of all requests):

| Operation | Share |
|---|---|
| `GET /posts?limit=20` | 18% |
| `GET /posts?limit=20&category=<random>` | 18% |
| `GET /posts/:id` (random known post) | 54% |
| `POST /posts/:id/comments` (random user, generated text) | 10% |
| `POST /posts` (random author and category, title marked `(load test xxxxxx)`) | 0% (`--new-post-share`; new posts would sit on the front page the talk shows next) |

**Refreshing the post pool.** Posts the generator creates join the id pool as
soon as their 201 arrives. Every `--refresh` seconds (15 by default) it also
re-reads the newest page from alternating Regions, so posts written through
the web UI get read too. A post created in one Region is read from both,
which exercises multi-Region strong consistency: a 404 there would be a bug.

## Load control

- **Open loop** (`--rps`): a pacer per Region issues requests on a 1 ms
  schedule at the controller's current rate, whatever the latency. If a
  Region reaches `--connections` requests in flight, further sends are
  *skipped and counted* rather than queued. A non-zero `skipped` means the
  API or the generator is saturated.
- **Closed loop** (`--concurrency`): N workers split across Regions by
  weight. The ramp grows the number of active workers.
- **Back-off happens only on throttling**, as in the riv25 helper:
  - A response counts as throttled if it is HTTP 429, or carries
    `x-amzn-ErrorType: TooManyRequestsException` (or `*Throttl*`).
  - Throttles are counted separately from other 4xx.
  - If at least 0.5% of a second's responses in a Region were throttled,
    that Region's rate is cut to 85%, held for 3 s, then raised by 2% of the
    target per second until it is back on the ramp.
  - 5xx, timeouts and connection errors are recorded but never slow the
    generator down.

## Quotas and connection-rate notes (why the ramp is gentle)

- Every new Lambda execution environment opens its own DSQL connection.
- A DSQL cluster accepts **100 new connections/s (burst 1,000)** and
  **10,000 connections** in total. Each Region has its own cluster, so each
  has its own limits.
- The talk account's Lambda concurrency is **10,000 in us-west-2 but 1,000 in
  us-east-1**, so us-east-1 is the ceiling until that quota is raised.

The arithmetic: at a steady state, concurrent executions per Region ≈
req/s per Region × seconds per request. While ramping, new environments
(that is, new DSQL connections) per second ≈ (target per Region ÷ ramp
seconds) × seconds per request.

| Total target | Per Region | Concurrency at 30 ms | New envs/s, 60 s ramp | New envs/s, `--ramp 0` |
|---|---|---|---|---|
| 2,000 | 1,000 | ~30 | ~0.5 | 30 at once |
| 20,000 | 10,000 | ~300 | ~5 | 300 at once |
| 40,000 | 20,000 | ~600 | ~10 | 600 at once |
| 66,000 | 33,000 | ~1,000 (account limit) | ~17 | 1,000 at once |

- A 60 s ramp keeps connection opens at a few per second, far below
  100/s, and makes the climb visible on screen.
- `--ramp 0` asks for every environment at once. Those cold starts open
  connections together and eat into the 1,000 burst.
- Cold starts are slower than warm requests, so the real numbers during a
  ramp are somewhat higher than the table. That's another reason to ramp.
- With 1,000 concurrent executions per Region, each Region tops out around
  30k req/s at 30 ms per request. Above that, expect 429s and the generator
  backing off. Going higher needs a Lambda quota increase.
- At the start, loadgen prints a warning (and puts it in the Events panel)
  when the plan exceeds about 80% of the Lambda limit, or when the ramp
  would open more than 50 connections/s per cluster.

## HTTP

- **Function URLs do not negotiate HTTP/2.** Checked 2026-10-14 with
  `curl --http2` and `openssl s_client -alpn h2,http/1.1` against
  `*.lambda-url.us-east-1.on.aws` and `*.lambda-url.us-east-2.on.aws`: the
  server picks no ALPN protocol and answers HTTP/1.1. So every in-flight
  request needs its own keep-alive connection.
- `--connections` is therefore both the connection cap and the in-flight cap
  per Region.
- The client is reqwest with rustls (aws-lc-rs) and one connection pool per
  Region, with keep-alive, TCP_NODELAY and no proxy.
- loadgen still offers h2 over ALPN, and reports the negotiated version at
  startup (`HTTP/1.1` in the plan line, and in the report). If a server
  ever speaks h2, requests are spread over ceil(N/100) multiplexed
  connections.
- `--http2-prior-knowledge` (h2c) exists for testing against the mock.
- loadgen raises its open-files soft limit to the hard limit at startup.

## Recommended invocations

**Rehearsal, laptop → real API (small).** From a laptop, latency is about
100 ms per call, so a few hundred req/s is plenty to check that everything
works:

```sh
./target/release/loadgen --rps 300 --ramp 20 --duration 2m
./target/release/loadgen --rps 300 --ramp 20 --duration 2m --plain --report rehearsal.json
```

**Rehearsal, laptop → local mock (no AWS).** The mock implements the spec
endpoints in memory, with random latency and an optional Lambda-style
concurrency limit that answers 429:

```sh
cargo run --release --example mock_server -- --region us-east-1 --region us-east-2 \
    --write-outputs /tmp/mock-outputs.json                    # add --max-concurrency 5 to see 429s and back-off
./target/release/loadgen --outputs /tmp/mock-outputs.json --rps 2000 --ramp 10
```

**Stage, EC2 close to the API (big).**
1. Start an instance in us-west-2 (c7i.4xlarge or c7g.4xlarge, Amazon Linux 2023).
2. Run [`ec2-setup.sh`](ec2-setup.sh).
3. `scp demo/outputs.json` up to it.
4. Run inside `tmux`, so a dropped SSH session doesn't stop the load:

```sh
./target/release/loadgen --rps 2000 --ramp 30 --duration 2m      # smoke test first
./target/release/loadgen --rps 20000                             # the finale: 60 s ramp, then hold
./target/release/loadgen --rps 20000 --ramp 90 --report finale.json
```

20,000 req/s is ~300 concurrent executions per Region, well inside today's
quota, with room for cold-start spikes. Use `+` live for a second climb.
Don't go past ~50,000 req/s total without a Lambda quota increase.

## Local mock and tests

```sh
cargo build --release
cargo clippy --all-targets -- -D warnings
cargo test                       # unit tests + end-to-end tests against the in-process mock
LOADGEN_PRINT_TUI=1 cargo test print_tui -- --nocapture   # print a sample frame of the display
```

The integration tests (`tests/integration.rs`) run the real binary against
mock servers (`tests/support/mock.rs`, shared with
`examples/mock_server.rs`). They check:
- two Regions from an outputs file, and `--url` with weights;
- the read/write mix;
- that 429s are counted apart from 4xx and trigger back-off;
- the closed-loop in-flight count and the `--connections` cap;
- h2c;
- bootstrap when posts share a timestamp, and an empty database.

**Laptop limits when using the mock.** On this laptop (macOS, 12 cores, busy
with other work), with the mock and loadgen sharing the machine:
- the closed loop peaked at about 37k req/s, with loadgen using about 2.2
  cores;
- the open loop held 20k req/s cleanly.

Pushing the open loop much harder against the local mock eventually makes
the macOS loopback stack collapse. The cascade:
1. A hiccup grows in-flight, which needs a burst of new connections.
2. The burst overflows the 128-entry accept backlog (`kern.ipc.somaxconn`).
3. SYN retransmits add 1 s or more to those requests.
4. Timed-out connections leave TIME_WAIT sockets, and the 16k ephemeral
   ports run out.

That is a property of the laptop and the mock, not of function URLs. For
local maximum-throughput tests, use `--concurrency`, which keeps a fixed set
of connections.
