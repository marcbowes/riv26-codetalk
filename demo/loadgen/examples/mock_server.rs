//! Local mock of the blog API for rehearsing the load generator without AWS.
//!
//!   cargo run --release --example mock_server -- --region mock-east-1 --region mock-east-2 \
//!       --write-outputs /tmp/mock-outputs.json
//!   cargo run --release -- --outputs /tmp/mock-outputs.json --rps 2000 --ramp 5
//!
//! Each --region gets its own port (starting at --port); all of them share one
//! in-memory dataset, like a multi-Region cluster.

#[path = "../tests/support/mock.rs"]
mod mock;

use std::net::SocketAddr;
use std::path::PathBuf;

use clap::Parser;

#[derive(Parser)]
#[command(about = "In-memory mock of the DAT401 blog API")]
struct Args {
    /// Region label; repeat for several servers on consecutive ports
    #[arg(long = "region", default_value = "mock-east-1")]
    regions: Vec<String>,
    /// First port
    #[arg(long, default_value_t = 8081)]
    port: u16,
    /// Address to bind
    #[arg(long, default_value = "127.0.0.1")]
    host: String,
    /// Simulated handler latency range in ms, e.g. 2-12
    #[arg(long, default_value = "2-12")]
    latency_ms: String,
    /// Answer 429 (TooManyRequestsException) beyond this many concurrent requests per server
    #[arg(long)]
    max_concurrency: Option<usize>,
    #[arg(long, default_value_t = 200)]
    users: usize,
    #[arg(long, default_value_t = 5000)]
    posts: usize,
    /// Write an outputs.json pointing at these servers
    #[arg(long)]
    write_outputs: Option<PathBuf>,
    /// Tokio worker threads
    #[arg(long)]
    threads: Option<usize>,
}

fn parse_range(s: &str) -> (u64, u64) {
    match s.split_once('-') {
        Some((a, b)) => (a.trim().parse().unwrap_or(0), b.trim().parse().unwrap_or(0)),
        None => {
            let v = s.trim().parse().unwrap_or(0);
            (v, v)
        }
    }
}

fn main() -> std::io::Result<()> {
    let args = Args::parse();
    let mut rt = tokio::runtime::Builder::new_multi_thread();
    if let Some(t) = args.threads {
        rt.worker_threads(t);
    }
    rt.enable_all().build()?.block_on(async move {
        let shared = mock::Shared::seed(mock::SeedConfig {
            users: args.users,
            posts: args.posts,
            same_timestamp_batch: 1,
        });
        let latency_ms = parse_range(&args.latency_ms);
        let mut regions = Vec::new();
        for (i, region) in args.regions.iter().enumerate() {
            let addr: SocketAddr = format!("{}:{}", args.host, args.port + i as u16)
                .parse()
                .expect("valid address");
            let cfg = mock::MockConfig {
                region: region.clone(),
                latency_ms,
                max_concurrency: args.max_concurrency,
            };
            let server = mock::start(cfg, shared.clone(), addr).await?;
            println!("{region}: {}", server.url());
            regions.push(serde_json::json!({ "region": region, "apiUrl": server.url() }));
        }
        if let Some(path) = &args.write_outputs {
            let doc = serde_json::json!({ "witnessRegion": "mock", "regions": regions });
            std::fs::write(path, serde_json::to_string_pretty(&doc).unwrap())?;
            println!("wrote {}", path.display());
        }
        println!(
            "{} users, {} posts, latency {}-{} ms{}; Ctrl-C to stop",
            args.users,
            args.posts,
            latency_ms.0,
            latency_ms.1,
            args.max_concurrency
                .map(|m| format!(", 429 above {m} in flight"))
                .unwrap_or_default()
        );
        tokio::signal::ctrl_c().await
    })
}
