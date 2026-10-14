//! loadgen: HTTP load generator for the DAT401 (re:Invent 2026) multi-Region
//! blog API. See README.md.

mod api;
mod app;
mod cli;
mod control;
mod mix;
mod monitor;
mod pool;
mod runner;
mod stats;
mod summary;
mod targets;
mod text;
mod tui;
mod util;

use std::process::ExitCode;
use std::time::Duration;

use clap::Parser;

fn main() -> ExitCode {
    let args = cli::Args::parse();
    let fd_limit = util::raise_fd_limit();
    let threads = args
        .threads
        .unwrap_or_else(|| {
            std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(4)
        })
        .max(1);
    let rt = match tokio::runtime::Builder::new_multi_thread()
        .worker_threads(threads)
        .thread_name("loadgen-worker")
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("error: failed to start the tokio runtime: {e}");
            return ExitCode::FAILURE;
        }
    };
    let result = rt.block_on(app::run(args, threads, fd_limit));
    // Don't wait for stragglers (e.g. DNS lookups on the blocking pool).
    rt.shutdown_timeout(Duration::from_secs(1));
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::FAILURE
        }
    }
}
