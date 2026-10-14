//! End-to-end: run the real `loadgen` binary against in-process mock servers.

mod support;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};

use serde_json::Value;
use support::mock::{self, MockConfig, MockServer, SeedConfig, Shared};

static RUN: AtomicUsize = AtomicUsize::new(0);

fn any_port() -> SocketAddr {
    "127.0.0.1:0".parse().unwrap()
}

fn scratch_dir() -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "loadgen-it-{}-{}",
        std::process::id(),
        RUN.fetch_add(1, Relaxed)
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

struct Run {
    ok: bool,
    stdout: String,
    stderr: String,
    report: Value,
}

async fn loadgen(args: &[&str]) -> Run {
    let dir = scratch_dir();
    let report = dir.join("report.json");
    let mut cmd = std::process::Command::new(env!("CARGO_BIN_EXE_loadgen"));
    cmd.args(args)
        .arg("--plain")
        .arg("--threads")
        .arg("2")
        .arg("--report")
        .arg(&report);
    let out = tokio::task::spawn_blocking(move || cmd.output())
        .await
        .unwrap()
        .expect("run loadgen");
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    let report = std::fs::read_to_string(&report)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or(Value::Null);
    let run = Run {
        ok: out.status.success(),
        stdout,
        stderr,
        report,
    };
    assert!(
        run.ok,
        "loadgen failed\nstdout:\n{}\nstderr:\n{}",
        run.stdout, run.stderr
    );
    run
}

async fn server(
    region: &str,
    shared: Arc<Shared>,
    tweak: impl FnOnce(&mut MockConfig),
) -> MockServer {
    let mut cfg = MockConfig::region(region);
    tweak(&mut cfg);
    mock::start(cfg, shared, any_port()).await.unwrap()
}

fn u(v: &Value) -> u64 {
    v.as_u64().unwrap_or_else(|| panic!("not a u64: {v}"))
}

fn errors_total(v: &Value) -> u64 {
    v.as_object().unwrap().values().map(u).sum()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_regions_from_outputs_file() {
    let shared = Shared::seed(SeedConfig::default());
    let a = server("mock-east-1", shared.clone(), |_| {}).await;
    let b = server("mock-east-2", shared.clone(), |_| {}).await;
    let outputs = scratch_dir().join("outputs.json");
    std::fs::write(
        &outputs,
        serde_json::json!({
            "witnessRegion": "mock",
            "regions": [
                { "region": "mock-east-1", "apiUrl": a.url(), "clusterId": "x" },
                { "region": "mock-east-2", "apiUrl": b.url().trim_end_matches('/'), "clusterId": "y" },
            ]
        })
        .to_string(),
    )
    .unwrap();

    let run = loadgen(&[
        "--outputs",
        outputs.to_str().unwrap(),
        "--rps",
        "400",
        "--ramp",
        "0.5",
        "--duration",
        "4s",
        "--bootstrap-posts",
        "1000",
        "--refresh",
        "1",
    ])
    .await;
    let r = &run.report;
    let totals = &r["totals"];
    let ok = u(&totals["ok"]);
    // ~400/s for ~3.75 s after the ramp.
    assert!((1200..=1700).contains(&ok), "ok {ok}");
    assert_eq!(errors_total(&totals["errors"]), 0, "{}", totals["errors"]);
    let writes = u(&totals["writes"]) as f64 / u(&totals["requests"]) as f64;
    assert!((0.06..=0.14).contains(&writes), "write share {writes}");

    let regions = r["regions"].as_array().unwrap();
    assert_eq!(regions[0]["label"], "mock-east-1");
    assert_eq!(regions[1]["label"], "mock-east-2");
    let share = u(&regions[0]["ok"]) as f64 / ok as f64;
    assert!((0.4..=0.6).contains(&share), "region split {share}");
    assert_eq!(regions[0]["httpVersion"], "HTTP/1.1");
    assert!(regions[0]["dbMs"]["p50"].as_f64().unwrap() > 0.0);
    assert!(u(&r["postPool"]) >= 1000);
    assert_eq!(u(&r["users"]), 200);

    // The mocks saw the whole mix, and writes landed in the shared "cluster".
    for s in [&a, &b] {
        let c = &s.state.counters;
        assert!(c.list.load(Relaxed) > 0);
        assert!(c.list_category.load(Relaxed) > 0);
        assert!(c.get_post.load(Relaxed) > 0);
        assert!(c.create_comment.load(Relaxed) > 0);
        assert_eq!(c.not_found.load(Relaxed), 0);
        assert_eq!(c.bad_request.load(Relaxed), 0);
    }
    assert!(shared.comments_created.load(Relaxed) > 50);

    assert!(
        run.stdout
            .contains("bootstrap from mock-east-1: 200 users, 8 categories"),
        "{}",
        run.stdout
    );
    let lines = run.stdout.lines().filter(|l| l.contains("] TPS")).count();
    assert!(lines >= 3, "plain lines: {lines}\n{}", run.stdout);
    assert!(run.stdout.contains("SUMMARY"));
    assert!(run.stdout.contains("POST /posts/:id/comments"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn urls_take_region_names_from_health_and_honor_weights() {
    let a = server("alpha", Shared::seed(SeedConfig::default()), |_| {}).await;
    let b = server("beta", Shared::seed(SeedConfig::default()), |_| {}).await;
    let run = loadgen(&[
        "--url",
        &a.url(),
        "--url",
        &b.url(),
        "--region-weights",
        "3,1",
        "--rps",
        "400",
        "--ramp",
        "0",
        "--duration",
        "3s",
        "--refresh",
        "0",
    ])
    .await;
    let regions = run.report["regions"].as_array().unwrap();
    assert_eq!(regions[0]["label"], "alpha");
    assert_eq!(regions[1]["label"], "beta");
    let (ra, rb) = (
        u(&regions[0]["requests"]) as f64,
        u(&regions[1]["requests"]) as f64,
    );
    let share = ra / (ra + rb);
    assert!((0.68..=0.82).contains(&share), "alpha share {share}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn throttling_is_counted_apart_and_backs_off() {
    // Capacity is about 4 / 15 ms = ~270 req/s; ask for 1,000.
    let s = server("throttly", Shared::seed(SeedConfig::default()), |c| {
        c.max_concurrency = Some(4);
        c.latency_ms = (10, 20);
    })
    .await;
    let run = loadgen(&[
        "--url",
        &s.url(),
        "--rps",
        "1000",
        "--ramp",
        "0",
        "--duration",
        "5s",
        "--refresh",
        "0",
    ])
    .await;
    let region = &run.report["regions"][0];
    let errors = &region["errors"];
    let throttled = u(&errors["throttled"]);
    assert!(throttled > 0);
    assert_eq!(u(&errors["client4xx"]), 0, "429s must not count as 4xx");
    assert_eq!(u(&errors["server5xx"]), 0);
    assert_eq!(throttled, s.state.counters.throttled.load(Relaxed));
    assert!(u(&region["backoffs"]) >= 1);
    let final_rate = region["finalRate"].as_f64().unwrap();
    assert!(final_rate < 700.0, "final rate {final_rate}");
    assert!(run.stdout.contains("backing off"), "{}", run.stdout);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn closed_loop_keeps_exactly_n_in_flight() {
    let s = server("closed", Shared::seed(SeedConfig::default()), |c| {
        c.latency_ms = (5, 10)
    })
    .await;
    let run = loadgen(&[
        "--url",
        &s.url(),
        "--concurrency",
        "6",
        "--ramp",
        "0",
        "--duration",
        "2s",
        "--refresh",
        "0",
    ])
    .await;
    let max = s.state.counters.max_in_flight.load(Relaxed);
    assert!((4..=6).contains(&max), "server saw {max} in flight");
    assert!(u(&run.report["totals"]["ok"]) > 200);
    assert_eq!(run.report["config"]["mode"], "concurrency");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn connection_cap_limits_in_flight_and_counts_skips() {
    let s = server("capped", Shared::seed(SeedConfig::default()), |c| {
        c.latency_ms = (20, 30)
    })
    .await;
    // 2,000/s x 25 ms needs ~50 in flight; allow 8.
    let run = loadgen(&[
        "--url",
        &s.url(),
        "--rps",
        "2000",
        "--ramp",
        "0",
        "--duration",
        "2s",
        "--connections",
        "8",
        "--refresh",
        "0",
    ])
    .await;
    let max = s.state.counters.max_in_flight.load(Relaxed);
    assert!(max <= 8, "server saw {max} in flight");
    let region = &run.report["regions"][0];
    assert!(u(&region["skipped"]) > 1000);
    assert_eq!(errors_total(&region["errors"]), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn http2_prior_knowledge_works() {
    let s = server("h2", Shared::seed(SeedConfig::default()), |_| {}).await;
    let run = loadgen(&[
        "--url",
        &s.url(),
        "--http2-prior-knowledge",
        "--rps",
        "300",
        "--ramp",
        "0",
        "--duration",
        "2s",
        "--refresh",
        "0",
    ])
    .await;
    let region = &run.report["regions"][0];
    assert_eq!(region["httpVersion"], "HTTP/2.0");
    assert!(u(&region["ok"]) > 400);
    assert_eq!(errors_total(&region["errors"]), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bootstrap_survives_posts_sharing_a_timestamp() {
    // Bulk loads in one transaction give every row the same created_at;
    // paging by ?before= then skips the rest of each batch. It must still end.
    let shared = Shared::seed(SeedConfig {
        users: 50,
        posts: 3000,
        same_timestamp_batch: 500,
    });
    let s = server("batched", shared, |_| {}).await;
    let run = loadgen(&[
        "--url",
        &s.url(),
        "--rps",
        "50",
        "--ramp",
        "0",
        "--duration",
        "1s",
        "--refresh",
        "0",
    ])
    .await;
    let pool = u(&run.report["postPool"]);
    assert!((600..3000).contains(&pool), "pool {pool}");
    assert_eq!(u(&run.report["users"]), 50);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn empty_database_still_runs() {
    let s = server(
        "empty",
        Shared::seed(SeedConfig {
            users: 5,
            posts: 0,
            same_timestamp_batch: 1,
        }),
        |_| {},
    )
    .await;
    let run = loadgen(&[
        "--url",
        &s.url(),
        "--rps",
        "200",
        "--ramp",
        "0",
        "--duration",
        "2s",
        "--refresh",
        "0",
    ])
    .await;
    assert!(run.stdout.contains("no posts yet"), "{}", run.stdout);
    // New posts get created and then read and commented on.
    assert!(s.state.counters.create_post.load(Relaxed) > 0);
    assert!(s.state.counters.get_post.load(Relaxed) > 0);
    assert_eq!(s.state.counters.not_found.load(Relaxed), 0);
    assert_eq!(errors_total(&run.report["totals"]["errors"]), 0);
}

#[test]
fn help_explains_the_gentle_ramp() {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_loadgen"))
        .arg("--help")
        .output()
        .unwrap();
    let help = String::from_utf8_lossy(&out.stdout);
    assert!(help.contains("100 new connections/s"), "{help}");
    assert!(help.contains("1,000 per Region"), "{help}");
    assert!(help.contains("HTTP/1.1"), "{help}");
}
