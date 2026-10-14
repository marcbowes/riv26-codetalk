//! End-of-run summary table and the optional JSON report.

use std::path::Path;
use std::sync::atomic::Ordering::Relaxed;

use anyhow::{Context, Result};
use serde_json::{Map, Value, json};

use crate::mix::Op;
use crate::monitor::Monitor;
use crate::runner::{Ctx, Mode};
use crate::stats::{Class, Interval, max_ms, pct_ms};
use crate::util::{fmt_int, fmt_ms, fmt_rate};

pub struct RunInfo {
    pub mode: Mode,
    pub target: f64,
    pub ramp: f64,
    pub duration: Option<f64>,
    pub read_pct: f64,
    pub write_pct: f64,
    pub mix_detail: String,
    pub connections: usize,
    pub threads: usize,
    pub timeout: f64,
    pub started_unix: f64,
    pub source: String,
}

fn table(header: &[&str], rows: &[Vec<String>]) -> String {
    let mut widths: Vec<usize> = header.iter().map(|h| h.chars().count()).collect();
    for r in rows {
        for (w, c) in widths.iter_mut().zip(r) {
            *w = (*w).max(c.chars().count());
        }
    }
    let fmt_row = |cells: Vec<String>| -> String {
        cells
            .iter()
            .enumerate()
            .map(|(i, c)| {
                if i == 0 {
                    format!("{c:<w$}", w = widths[i])
                } else {
                    format!("{c:>w$}", w = widths[i])
                }
            })
            .collect::<Vec<_>>()
            .join("  ")
            .trim_end()
            .to_string()
    };
    let mut out = fmt_row(header.iter().map(|s| s.to_string()).collect());
    out.push('\n');
    for r in rows {
        out.push_str(&fmt_row(r.clone()));
        out.push('\n');
    }
    out
}

fn region_row(
    label: &str,
    i: &Interval,
    secs: f64,
    peak: f64,
    skipped: u64,
    show_db: bool,
) -> Vec<String> {
    let mut row = vec![
        label.to_string(),
        fmt_int(i.total()),
        fmt_int(i.ok()),
        fmt_rate(i.ok() as f64 / secs.max(1e-9)),
        fmt_rate(peak),
        fmt_ms(pct_ms(&i.lat, 0.5)),
        fmt_ms(pct_ms(&i.lat, 0.9)),
        fmt_ms(pct_ms(&i.lat, 0.99)),
        fmt_ms(max_ms(&i.lat)),
    ];
    if show_db {
        row.push(fmt_ms(pct_ms(&i.db, 0.5)));
        row.push(fmt_ms(pct_ms(&i.db, 0.99)));
    }
    for c in Class::ERRORS {
        row.push(fmt_int(i.count(c)));
    }
    row.push(fmt_int(skipped));
    row
}

pub fn print(info: &RunInfo, ctx: &Ctx, mon: &Monitor, load_secs: f64) {
    let bar = "=".repeat(78);
    let mode = match info.mode {
        Mode::Rate => format!("open loop, target {} req/s", fmt_rate(info.target)),
        Mode::Concurrency => format!("closed loop, {} workers", fmt_int(info.target as u64)),
    };
    println!();
    println!("{bar}");
    println!(
        "SUMMARY  {:.1} s of load · {mode} · ramp {} s · mix {}/{} read/write",
        load_secs, info.ramp, info.read_pct, info.write_pct
    );
    println!("{bar}");
    let mut header = vec![
        "Region", "requests", "2xx", "avg TPS", "peak TPS", "p50 ms", "p90 ms", "p99 ms", "max ms",
    ];
    if mon.db_seen {
        header.push("DB p50");
        header.push("DB p99");
    }
    header.extend(["4xx", "429", "5xx", "timeout", "connect", "skipped"]);
    let mut rows = Vec::new();
    let mut skipped_all = 0;
    for (t, target) in ctx.targets.iter().enumerate() {
        let skipped = target.skipped.load(Relaxed);
        skipped_all += skipped;
        rows.push(region_row(
            &target.label,
            &mon.totals[t],
            load_secs,
            mon.peak[t],
            skipped,
            mon.db_seen,
        ));
    }
    let all = mon.totals_all();
    if ctx.targets.len() > 1 {
        rows.push(region_row(
            "all",
            &all,
            load_secs,
            mon.peak_total,
            skipped_all,
            mon.db_seen,
        ));
    }
    print!("{}", table(&header, &rows));
    println!("(latency and DB time are for successful requests; TPS = 2xx responses per second)");
    println!();

    let mut op_rows = Vec::new();
    for op in Op::ALL {
        let n: u64 = mon.totals.iter().map(|t| t.op_total(op)).sum();
        if n == 0 {
            continue;
        }
        let ok: u64 = mon
            .totals
            .iter()
            .map(|t| t.op_counts[op.index()][Class::Ok.index()])
            .sum();
        let mut h = crate::stats::new_hist();
        for t in &mon.totals {
            if !t.op_lat[op.index()].is_empty() {
                h.add(&t.op_lat[op.index()]).expect("same bounds");
            }
        }
        op_rows.push(vec![
            op.label().to_string(),
            fmt_int(n),
            format!("{:.1}%", 100.0 * n as f64 / all.total().max(1) as f64),
            fmt_int(ok),
            fmt_int(n - ok),
            fmt_ms(pct_ms(&h, 0.5)),
            fmt_ms(pct_ms(&h, 0.99)),
        ]);
    }
    print!(
        "{}",
        table(
            &[
                "Operation",
                "requests",
                "share",
                "2xx",
                "errors",
                "p50 ms",
                "p99 ms"
            ],
            &op_rows
        )
    );
    println!();
    let backoffs: u64 = (0..ctx.targets.len())
        .map(|t| ctx.controller(t).backoffs())
        .sum();
    println!(
        "Throttle back-offs: {backoffs}   Post id pool: {}   Users: {}",
        fmt_int(ctx.pool.post_count() as u64),
        fmt_int(ctx.pool.user_count() as u64)
    );
}

fn errors_json(i: &Interval) -> Value {
    let mut m = Map::new();
    for c in Class::ERRORS {
        m.insert(c.key().into(), json!(i.count(c)));
    }
    Value::Object(m)
}

fn latency_json(i: &Interval) -> Value {
    json!({
        "p50": pct_ms(&i.lat, 0.5),
        "p90": pct_ms(&i.lat, 0.9),
        "p99": pct_ms(&i.lat, 0.99),
        "p999": pct_ms(&i.lat, 0.999),
        "max": max_ms(&i.lat),
    })
}

fn db_json(i: &Interval) -> Value {
    json!({ "p50": pct_ms(&i.db, 0.5), "p99": pct_ms(&i.db, 0.99), "samples": i.db.len() })
}

pub fn report(info: &RunInfo, ctx: &Ctx, mon: &Monitor, load_secs: f64) -> Value {
    let all = mon.totals_all();
    let regions: Vec<Value> = ctx
        .targets
        .iter()
        .enumerate()
        .map(|(t, target)| {
            let i = &mon.totals[t];
            let c = ctx.controller(t);
            let mut ops = Map::new();
            for op in Op::ALL {
                let n = i.op_total(op);
                let ok = i.op_counts[op.index()][Class::Ok.index()];
                ops.insert(
                    op.key().into(),
                    json!({
                        "requests": n,
                        "ok": ok,
                        "errors": n - ok,
                        "p50Ms": pct_ms(&i.op_lat[op.index()], 0.5),
                        "p99Ms": pct_ms(&i.op_lat[op.index()], 0.99),
                    }),
                );
            }
            json!({
                "label": target.label,
                "url": target.base,
                "httpVersion": format!("{:?}", target.version),
                "requests": i.total(),
                "ok": i.ok(),
                "avgTps": i.ok() as f64 / load_secs.max(1e-9),
                "peakTps": mon.peak[t],
                "maxInFlight": mon.max_in_flight[t],
                "connectionCap": target.cap,
                "skipped": target.skipped.load(Relaxed),
                "backoffs": c.backoffs(),
                "finalTarget": c.target(),
                "finalRate": c.rate(load_secs),
                "workers": ctx.workers.get(t).copied().unwrap_or(0),
                "errors": errors_json(i),
                "latencyMs": latency_json(i),
                "dbMs": db_json(i),
                "ops": Value::Object(ops),
            })
        })
        .collect();
    json!({
        "tool": concat!("dat401-loadgen ", env!("CARGO_PKG_VERSION")),
        "startedAtUnix": info.started_unix,
        "loadSecs": load_secs,
        "config": {
            "mode": match info.mode { Mode::Rate => "rps", Mode::Concurrency => "concurrency" },
            "target": info.target,
            "rampSecs": info.ramp,
            "durationSecs": info.duration,
            "mix": { "read": info.read_pct, "write": info.write_pct, "detail": info.mix_detail },
            "connectionsPerRegion": info.connections,
            "threads": info.threads,
            "timeoutSecs": info.timeout,
            "source": info.source,
        },
        "totals": {
            "requests": all.total(),
            "ok": all.ok(),
            "avgTps": all.ok() as f64 / load_secs.max(1e-9),
            "peakTps": mon.peak_total,
            "writes": Op::ALL.iter().filter(|op| op.is_write()).map(|op| all.op_total(*op)).sum::<u64>(),
            "errors": errors_json(&all),
            "latencyMs": latency_json(&all),
            "dbMs": db_json(&all),
        },
        "regions": regions,
        "postPool": ctx.pool.post_count(),
        "users": ctx.pool.user_count(),
        "events": ctx.events.recent(200),
        "timeline": mon.timeline,
    })
}

pub fn write_report(path: &Path, report: &Value) -> Result<()> {
    let text = serde_json::to_string_pretty(report)?;
    std::fs::write(path, text).with_context(|| format!("writing {}", path.display()))
}
