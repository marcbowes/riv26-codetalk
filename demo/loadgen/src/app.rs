//! Wiring: resolve targets, health-check, bootstrap ids, run, summarise.

use std::collections::{BTreeSet, HashMap};
use std::io::IsTerminal;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::Ordering::Relaxed;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use reqwest::{Client, Version};
use tokio::task::JoinSet;
use tokio::time::MissedTickBehavior;

use crate::api;
use crate::cli::Args;
use crate::control::{Adjust, Controller};
use crate::mix::{Mix, MixConfig};
use crate::monitor::{Monitor, TICK, ViewInfo};
use crate::pool::IdPool;
use crate::runner::{self, Ctx, EventLog, HttpSettings, Mode, Padded, Proto, Target};
use crate::stats::{Class, Stats};
use crate::summary::{self, RunInfo};
use crate::targets::{self, TargetSpec};
use crate::tui::{Command, Ui};
use crate::util::{fmt_int, fmt_rate};

/// Assumed time per request (Lambda + one DSQL transaction) for the warnings.
const EST_REQUEST_SECS: f64 = 0.030;
const LAMBDA_CONCURRENCY_PER_REGION: f64 = 1000.0;
const DSQL_NEW_CONNECTIONS_PER_SEC: f64 = 100.0;
const POOL_CAP: usize = 20_000;

fn secs(v: f64) -> Duration {
    Duration::from_secs_f64(v)
}

fn validate(a: &Args) -> Result<()> {
    if let Some(r) = a.rps
        && !(r.is_finite() && r > 0.0)
    {
        bail!("--rps must be a positive number");
    }
    if a.concurrency == Some(0) {
        bail!("--concurrency must be at least 1");
    }
    if !(a.ramp.is_finite() && a.ramp >= 0.0) {
        bail!("--ramp must be zero or more seconds");
    }
    let positive = |v: f64| v.is_finite() && v > 0.0;
    if !positive(a.timeout) || !positive(a.connect_timeout) {
        bail!("--timeout and --connect-timeout must be positive");
    }
    if a.connections == 0 {
        bail!("--connections must be at least 1");
    }
    if a.page_size == 0 {
        bail!("--page-size must be at least 1");
    }
    if !(a.refresh.is_finite() && a.refresh >= 0.0) {
        bail!("--refresh must be zero or more seconds");
    }
    Ok(())
}

fn resolve_targets(a: &Args) -> Result<(Vec<TargetSpec>, String)> {
    if !a.urls.is_empty() {
        return Ok((targets::from_urls(&a.urls)?, "--url".into()));
    }
    if let Some(p) = &a.outputs {
        return Ok((targets::from_outputs(p)?, p.display().to_string()));
    }
    for p in targets::default_outputs_candidates() {
        if p.exists() {
            return Ok((targets::from_outputs(&p)?, p.display().to_string()));
        }
    }
    bail!(
        "no targets: ../outputs.json not found (run demo/deploy.sh first), \
         or pass --outputs PATH or --url URL"
    )
}

async fn check_health(client: &Client, specs: &[TargetSpec]) -> Vec<Result<api::Health>> {
    let mut set = JoinSet::new();
    for (i, s) in specs.iter().enumerate() {
        let client = client.clone();
        let base = s.base.clone();
        set.spawn(async move { (i, api::health(&client, &base).await) });
    }
    let mut out: Vec<Option<Result<api::Health>>> = (0..specs.len()).map(|_| None).collect();
    while let Some(r) = set.join_next().await {
        if let Ok((i, h)) = r {
            out[i] = Some(h);
        }
    }
    out.into_iter()
        .map(|h| h.unwrap_or_else(|| Err(anyhow::anyhow!("health check task failed"))))
        .collect()
}

fn dedupe_labels(labels: &mut [String], specs: &[TargetSpec]) {
    let mut counts: HashMap<String, usize> = HashMap::new();
    for l in labels.iter() {
        *counts.entry(l.clone()).or_default() += 1;
    }
    for (l, s) in labels.iter_mut().zip(specs) {
        if counts[l.as_str()] > 1 {
            *l = format!("{l}@{}", targets::host_label(&s.base));
        }
    }
}

fn unit(mode: Mode) -> &'static str {
    match mode {
        Mode::Rate => " req/s",
        Mode::Concurrency => " workers",
    }
}

fn describe(label: &str, adj: Adjust, mode: Mode) -> String {
    let u = unit(mode);
    match adj {
        Adjust::BackedOff {
            from,
            to,
            throttled,
            completed,
        } => format!(
            "{label}: {} of {} responses throttled in the last second; backing off {}{u} -> {}{u}",
            fmt_int(throttled),
            fmt_int(completed),
            fmt_rate(from),
            fmt_rate(to)
        ),
        Adjust::Recovered { rate } => {
            format!(
                "{label}: no more throttling; back on the ramp at {}{u}",
                fmt_rate(rate)
            )
        }
    }
}

fn retarget(ctx: &Ctx, cmd: Command, t: f64) {
    let factor = if cmd == Command::Faster { 1.25 } else { 0.8 };
    let mut total = 0.0;
    let mut capped = false;
    for i in 0..ctx.targets.len() {
        let mut c = ctx.controller(i);
        let mut new = c.target() * factor;
        if ctx.mode == Mode::Concurrency {
            let max = ctx.workers[i] as f64;
            if new.round() > max {
                capped = true;
            }
            new = new.round().min(max).max(max.min(1.0));
        }
        c.set_target(t, new);
        total += new;
    }
    let mut msg = format!("target set to {}{}", fmt_rate(total), unit(ctx.mode));
    if capped {
        msg.push_str(" (capped at --concurrency)");
    }
    ctx.events.push(msg);
}

#[allow(clippy::too_many_lines)]
pub async fn run(args: Args, threads: usize, fd_limit: Option<u64>) -> Result<()> {
    validate(&args)?;
    let mode = if args.concurrency.is_some() {
        Mode::Concurrency
    } else {
        Mode::Rate
    };
    let total_target = match args.concurrency {
        Some(c) => c as f64,
        None => args.rps.unwrap_or(100.0),
    };
    let mix = Mix::new(&MixConfig {
        read_pct: args.mix.read,
        list_pct: args.list_share,
        category_pct: args.category_share,
        new_post_pct: args.new_post_share,
    })
    .map_err(anyhow::Error::msg)?;
    let plain = args.plain || !std::io::stdout().is_terminal();
    let http = HttpSettings {
        timeout: secs(args.timeout),
        connect_timeout: secs(args.connect_timeout),
        proto: if args.http1 {
            Proto::Http1
        } else if args.http2_prior_knowledge {
            Proto::H2c
        } else {
            Proto::Auto
        },
    };

    // Targets and health.
    let (specs, source) = resolve_targets(&args)?;
    println!(
        "loadgen {}: {} target(s) from {source}",
        env!("CARGO_PKG_VERSION"),
        specs.len()
    );
    let probe = http.client(8)?;
    let healths = check_health(&probe, &specs).await;
    let mut labels = Vec::new();
    let mut versions = Vec::new();
    let mut healthy = Vec::new();
    for (i, (spec, h)) in specs.iter().zip(&healths).enumerate() {
        match h {
            Ok(h) => {
                let label = spec
                    .label
                    .clone()
                    .or_else(|| h.region.clone())
                    .unwrap_or_else(|| targets::host_label(&spec.base));
                println!(
                    "  {label:<12} {}  {:?}  region={}  health {:.0} ms",
                    spec.base,
                    h.version,
                    h.region.as_deref().unwrap_or("?"),
                    h.latency.as_secs_f64() * 1000.0
                );
                if let (Some(want), Some(got)) = (&spec.label, &h.region)
                    && want != got
                {
                    println!("  warning: {want} answered as region {got:?}");
                }
                labels.push(label);
                versions.push(h.version);
                healthy.push(i);
            }
            Err(e) => {
                let label = spec
                    .label
                    .clone()
                    .unwrap_or_else(|| targets::host_label(&spec.base));
                println!("  {label:<12} {}  health check FAILED: {e:#}", spec.base);
                labels.push(label);
                versions.push(Version::HTTP_11);
            }
        }
    }
    if healthy.is_empty() {
        bail!("no target passed GET /health; is the API deployed and the URL right?");
    }
    dedupe_labels(&mut labels, &specs);
    let n = specs.len();
    let weights = match &args.region_weights {
        Some(s) => targets::parse_weights(s, &labels)?,
        None => vec![1.0; n],
    };

    // Bootstrap ids from the first healthy Region (multi-Region clusters
    // are strongly consistent, so any Region sees the same data).
    let boot_started = Instant::now();
    let boot = &specs[healthy[0]].base;
    let users = api::users(&probe, boot, 200).await.unwrap_or_else(|e| {
        println!("  warning: GET /users failed ({e:#}); writes are disabled");
        Vec::new()
    });
    let mut categories = api::categories(&probe, boot).await.unwrap_or_else(|e| {
        println!("  warning: GET /categories failed ({e:#}); using categories seen on posts");
        Vec::new()
    });
    let posts = api::collect_posts(&probe, boot, args.bootstrap_posts, &categories)
        .await
        .context("collecting post ids")?;
    if categories.is_empty() {
        categories = posts
            .iter()
            .filter_map(|p| p.category.clone())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
    }
    println!(
        "bootstrap from {}: {} users, {} categories, {} posts in {:.1} s",
        labels[healthy[0]],
        users.len(),
        categories.len(),
        fmt_int(posts.len() as u64),
        boot_started.elapsed().as_secs_f64()
    );
    if posts.is_empty() {
        println!(
            "  warning: no posts yet; reads of single posts and comments start once POST /posts creates some"
        );
    }
    let pool = IdPool::new(
        users,
        posts.into_iter().map(|p| p.id).collect(),
        POOL_CAP.max(args.bootstrap_posts * 2),
    );

    // Per-Region plan.
    let rates = targets::split(total_target, &weights);
    let workers: Vec<usize> = match mode {
        Mode::Concurrency => targets::split_workers(total_target as usize, &weights)
            .into_iter()
            .map(|w| w.min(args.connections))
            .collect(),
        Mode::Rate => vec![0; n],
    };
    let controllers = (0..n)
        .map(|t| {
            Mutex::new(match mode {
                Mode::Rate => Controller::new(rates[t], args.ramp, rates[t].min(1.0)),
                Mode::Concurrency => Controller::new(workers[t] as f64, args.ramp, 1.0),
            })
        })
        .collect();
    let target_list = (0..n)
        .map(|t| {
            Target::new(
                labels[t].clone(),
                specs[t].base.clone(),
                versions[t],
                &http,
                args.connections,
                &categories,
                args.page_size,
            )
        })
        .collect::<Result<Vec<_>>>()?;

    let per_region = |vals: &[f64]| {
        labels
            .iter()
            .zip(vals)
            .map(|(l, v)| format!("{l} {}", fmt_rate(*v)))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let (load, split) = match mode {
        Mode::Rate => (
            format!("open loop, {} req/s", fmt_rate(total_target)),
            per_region(&rates),
        ),
        Mode::Concurrency => (
            format!("closed loop, {} workers", fmt_int(total_target as u64)),
            per_region(&workers.iter().map(|w| *w as f64).collect::<Vec<_>>()),
        ),
    };
    let length = match args.duration {
        Some(d) => format!("for {:.0} s", d.as_secs_f64()),
        None => "until Ctrl-C".into(),
    };
    let http_label = {
        let set: BTreeSet<String> = versions.iter().map(|v| format!("{v:?}")).collect();
        set.into_iter().collect::<Vec<_>>().join("+")
    };
    println!("plan: {load} ({split}), ramp {} s, {length}", args.ramp);
    println!(
        "mix:  {}/{} read/write: {}",
        args.mix.read,
        args.mix.write,
        mix.describe()
    );
    println!(
        "http: {http_label}, up to {} connections per Region, timeout {} s, {threads} threads",
        fmt_int(args.connections as u64),
        args.timeout
    );

    let mut warnings = Vec::new();
    if mode == Mode::Rate {
        for t in 0..n {
            let r = rates[t];
            let envs = r * EST_REQUEST_SECS;
            if envs > LAMBDA_CONCURRENCY_PER_REGION * 0.8 {
                warnings.push(format!(
                    "{}: {} req/s needs ~{} concurrent Lambda executions at ~30 ms per request; \
                     the account limit is 1,000 per Region, so expect 429s (loadgen backs off)",
                    labels[t],
                    fmt_rate(r),
                    fmt_int(envs as u64)
                ));
            }
            if envs > 50.0 {
                if args.ramp == 0.0 {
                    warnings.push(format!(
                        "{}: --ramp 0 starts at {} req/s at once: ~{} cold starts opening DSQL \
                         connections together (a cluster accepts 100/s, burst 1,000)",
                        labels[t],
                        fmt_rate(r),
                        fmt_int(envs as u64)
                    ));
                } else {
                    let per_sec = r / args.ramp * EST_REQUEST_SECS;
                    if per_sec > DSQL_NEW_CONNECTIONS_PER_SEC * 0.5 {
                        warnings.push(format!(
                            "{}: this ramp adds ~{:.0} Lambda environments (= new DSQL connections) \
                             per second; a cluster accepts 100/s (burst 1,000). Consider --ramp {:.0} or more",
                            labels[t],
                            per_sec,
                            (envs / (DSQL_NEW_CONNECTIONS_PER_SEC * 0.5)).ceil()
                        ));
                    }
                }
            }
            if r * 0.1 > args.connections as f64 {
                warnings.push(format!(
                    "{}: --connections {} caps in-flight requests; {} req/s at 100 ms latency needs {}",
                    labels[t],
                    args.connections,
                    fmt_rate(r),
                    fmt_int((r * 0.1) as u64)
                ));
            }
        }
    }
    if let Some(limit) = fd_limit
        && limit < (args.connections * n + 256) as u64
    {
        warnings.push(format!(
            "open-files limit is {limit}, below --connections x Regions; raise it with ulimit -n"
        ));
    }

    // Go.
    let start = Instant::now();
    let started_unix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0);
    let ctx = Arc::new(Ctx {
        targets: target_list,
        pool,
        categories,
        mix,
        stats: Stats::new(n, threads),
        controllers,
        workers: workers.clone(),
        active: (0..n).map(|_| Padded(AtomicUsize::new(0))).collect(),
        mode,
        start,
        stop: AtomicBool::new(false),
        events: EventLog::new(start, plain),
    });
    for w in &warnings {
        ctx.events.push(format!("warning: {w}"));
    }
    ctx.events.push(format!(
        "ramping to {}{} over {} s",
        fmt_rate(total_target),
        unit(mode),
        args.ramp
    ));

    let mut tasks = JoinSet::new();
    for t in 0..n {
        match mode {
            Mode::Rate if rates[t] > 0.0 => {
                tasks.spawn(runner::pacer(ctx.clone(), t));
            }
            Mode::Concurrency => {
                for w in 0..workers[t] {
                    tasks.spawn(runner::worker(ctx.clone(), t, w));
                }
            }
            Mode::Rate => {}
        }
    }
    let refresher = (args.refresh > 0.0).then(|| {
        tokio::spawn(runner::refresher(
            ctx.clone(),
            probe.clone(),
            secs(args.refresh),
        ))
    });

    let (cmd_tx, mut cmd_rx) = tokio::sync::mpsc::unbounded_channel();
    {
        let cmd_tx = cmd_tx.clone();
        tokio::spawn(async move {
            while tokio::signal::ctrl_c().await.is_ok() {
                if cmd_tx.send(Command::Quit).is_err() {
                    break;
                }
            }
        });
    }
    let mut ui = (!plain).then(|| Ui::start(cmd_tx.clone()));
    let info = ViewInfo {
        title: args.title.clone(),
        duration: args.duration.map(|d| d.as_secs_f64()),
        mix: format!("{}/{} read/write", args.mix.read, args.mix.write),
        http: http_label,
    };

    let mut monitor = Monitor::new(n, start);
    let mut interval = tokio::time::interval(TICK);
    interval.set_missed_tick_behavior(MissedTickBehavior::Delay);
    interval.tick().await;
    let deadline = args.duration.map(|d| start + d);
    let mut quits = 0;
    loop {
        interval.tick().await;
        let now = Instant::now();
        let t_now = ctx.secs(now);
        let second = monitor.tick(&ctx, now);
        while let Ok(cmd) = cmd_rx.try_recv() {
            match cmd {
                Command::Quit => quits += 1,
                other => retarget(&ctx, other, t_now),
            }
        }
        if second {
            for t in 0..n {
                let w = monitor.window(t);
                let (throttled, completed) = (w.count(Class::Throttled), w.total());
                let adj = ctx.controller(t).observe(t_now, throttled, completed);
                if let Some(adj) = adj {
                    ctx.events.push(describe(&ctx.targets[t].label, adj, mode));
                }
            }
            monitor.record_second(&ctx, t_now);
            if plain {
                println!("{}", monitor.plain_line(&ctx, t_now));
            }
        }
        if mode == Mode::Concurrency {
            for (t, (spawned, active)) in workers.iter().zip(&ctx.active).enumerate() {
                let want = ctx.controller(t).rate(t_now).round() as usize;
                active.0.store(want.max(1).min(*spawned), Relaxed);
            }
        }
        if let Some(ui) = &ui {
            ui.show(monitor.view(&ctx, &info, now, None));
        }
        if quits > 0 || deadline.is_some_and(|d| now >= d) {
            break;
        }
    }

    // Stop sending, let in-flight requests finish (a second Ctrl-C/q skips this).
    let load_secs = ctx.secs(Instant::now());
    ctx.stop.store(true, Relaxed);
    if let Some(r) = refresher {
        r.abort();
    }
    let drain_until =
        Instant::now() + (http.timeout + Duration::from_secs(1)).min(Duration::from_secs(15));
    if plain && ctx.in_flight() > 0 {
        println!(
            "# stopping; waiting for {} in-flight request(s)",
            ctx.in_flight()
        );
    }
    while ctx.in_flight() > 0 && Instant::now() < drain_until && quits < 2 {
        tokio::time::sleep(Duration::from_millis(100)).await;
        while let Ok(cmd) = cmd_rx.try_recv() {
            if cmd == Command::Quit {
                quits += 1;
            }
        }
        if let Some(ui) = &ui {
            ui.show(monitor.view(&ctx, &info, Instant::now(), Some(ctx.in_flight())));
        }
    }
    tasks.abort_all();
    monitor.tick(&ctx, Instant::now());
    if let Some(mut ui) = ui.take() {
        ui.stop();
    }

    let run_info = RunInfo {
        mode,
        target: total_target,
        ramp: args.ramp,
        duration: args.duration.map(|d| d.as_secs_f64()),
        read_pct: args.mix.read,
        write_pct: args.mix.write,
        mix_detail: ctx.mix.describe(),
        connections: args.connections,
        threads,
        timeout: args.timeout,
        started_unix,
        source,
    };
    summary::print(&run_info, &ctx, &monitor, load_secs);
    if let Some(path) = &args.report {
        let report = summary::report(&run_info, &ctx, &monitor, load_secs);
        summary::write_report(path, &report)?;
        println!("report written to {}", path.display());
    }
    Ok(())
}
