//! `events`: tail Aurora DSQL CDC records from a Kinesis data stream and show how long each
//! change took to get here, stage by stage.

mod app;
mod chunks;
mod cli;
mod config;
mod format;
mod kinesis;
mod lag;
mod metrics;
mod pipeline;
mod record;
mod replay;
mod stats;
mod style;
mod tail;

use std::time::Duration;

use anyhow::{Context, Result, bail};
use aws_config::BehaviorVersion;
use aws_config::timeout::TimeoutConfig;
use clap::Parser;
use tokio::sync::mpsc;

use crate::app::{AppOptions, View};
use crate::cli::{Cli, Command, DisplayArgs, FilterArgs, SourceArgs};
use crate::metrics::Metrics;
use crate::pipeline::{Pipeline, PipelineOptions};
use crate::stats::{StatsOptions, StatsView};
use crate::style::{Painter, dim, warn};
use crate::tail::{TailOptions, TailView};

/// Batches buffered between the shard pollers and the display loop. When it's full, pollers
/// wait, and the backlog shows up as MillisBehindLatest rather than as memory.
const CHANNEL_DEPTH: usize = 256;

const TAIL_WINDOW_SECS: u64 = 60;
const STATS_WINDOW_SECS: u64 = 10;

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command.unwrap_or(Command::Tail(cli.tail)) {
        Command::Tail(a) => {
            let opts = TailOptions {
                json: a.json,
                color: a.display.color,
                quiet: a.display.quiet,
                live: true,
            };
            let view = move |_title: String| -> Box<dyn View> { Box::new(TailView::new(opts)) };
            live(
                &a.source,
                &a.filter,
                &a.display,
                a.json,
                TAIL_WINDOW_SECS,
                view,
            )
            .await
        }
        Command::Stats(a) => {
            let color = a.display.color;
            let view = move |title: String| -> Box<dyn View> {
                Box::new(StatsView::new(StatsOptions {
                    color,
                    title,
                    live: true,
                }))
            };
            live(
                &a.source,
                &a.filter,
                &a.display,
                false,
                STATS_WINDOW_SECS,
                view,
            )
            .await
        }
        Command::Replay(a) => {
            let window = a.display.window.unwrap_or(if a.stats {
                STATS_WINDOW_SECS
            } else {
                TAIL_WINDOW_SECS
            });
            let mut view: Box<dyn View> = if a.stats {
                Box::new(StatsView::new(StatsOptions {
                    color: a.display.color,
                    title: format!("replay {}", a.file.display()),
                    live: false,
                }))
            } else {
                Box::new(TailView::new(TailOptions {
                    json: a.json,
                    color: a.display.color,
                    quiet: a.display.quiet,
                    live: false,
                }))
            };
            if !a.file.is_file() {
                bail!("{}: no such file", a.file.display());
            }
            let (tx, rx) = mpsc::channel(CHANNEL_DEPTH);
            let opts = replay::ReplayOptions {
                speed: a.speed.max(0.0),
                max_gap: a.max_gap,
            };
            let file = a.file.clone();
            let reader = tokio::spawn(async move { replay::run(&file, opts, tx).await });
            let pipeline = Pipeline::new(pipeline_options(&a.filter, a.json));
            app::run(
                rx,
                pipeline,
                view.as_mut(),
                Metrics::new(window),
                AppOptions {
                    live: false,
                    tick: Duration::from_secs(1),
                    save: None,
                },
            )
            .await?;
            drop(view);
            reader.abort();
            Ok(())
        }
    }
}

fn pipeline_options(f: &FilterArgs, json: bool) -> PipelineOptions {
    PipelineOptions {
        tables: f
            .tables
            .iter()
            .map(|t| t.trim().to_owned())
            .filter(|t| !t.is_empty())
            .collect(),
        keep_raw: json,
        chunk_ttl: f.chunk_ttl,
    }
}

async fn live(
    source: &SourceArgs,
    filter: &FilterArgs,
    display: &DisplayArgs,
    json: bool,
    default_window: u64,
    make_view: impl FnOnce(String) -> Box<dyn View>,
) -> Result<()> {
    let stream = source
        .stream
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("pass --stream <name|arn> (the Kinesis stream your CDC stream writes to)"))?;
    let target = config::resolve_target(stream, source.region.as_deref())?;

    let mut loader = aws_config::defaults(BehaviorVersion::latest()).timeout_config(
        TimeoutConfig::builder()
            .connect_timeout(Duration::from_secs(5))
            .operation_timeout(Duration::from_secs(20))
            .build(),
    );
    if let Some(r) = &target.region {
        loader = loader.region(aws_config::Region::new(r.clone()));
    }
    if let Some(p) = &source.profile {
        loader = loader.profile_name(p);
    }
    let sdk = loader.load().await;
    let region = sdk.region().map(|r| r.to_string()).context(
        "no AWS Region: pass --region, use a stream ARN, or set AWS_REGION / a profile region",
    )?;
    let client = aws_sdk_kinesis::Client::new(&sdk);

    let info = kinesis::describe(&client, &target.stream)
        .await
        .with_context(|| format!("stream from {} in {region}", target.origin))?;
    let epaint = Painter::for_stream(display.color, &std::io::stderr());
    eprintln!(
        "{}",
        epaint.paint(
            &format!(
                "events: {} in {region} ({}, {}, {} open shards, {}h retention) from {} · via {} · Ctrl-C to stop",
                info.name,
                info.status,
                info.mode,
                info.open_shards,
                info.retention_hours,
                source.from.describe(),
                target.origin
            ),
            dim()
        )
    );
    if info.max_record_kib.is_some_and(|k| k < 10_240) {
        eprintln!(
            "{}",
            epaint.paint(
                &format!(
                    "! MaxRecordSizeInKiB is {}; DSQL CDC needs 10240 or large records stall the stream (KINESIS_OVERSIZE_RECORD)",
                    info.max_record_kib.unwrap_or_default()
                ),
                warn()
            )
        );
    }

    let (tx, rx) = mpsc::channel(CHANNEL_DEPTH);
    let reader = tokio::spawn(kinesis::run(
        kinesis::ReaderConfig {
            client,
            stream: config::StreamRef {
                name: None,
                arn: Some(info.arn.clone()),
            },
            from: source.from.clone(),
            poll: Duration::from_millis(source.poll_ms),
            relist: source.relist,
        },
        tx,
    ));

    let title = format!("{} ({region}) from {}", info.name, source.from.describe());
    let mut view = make_view(title);
    let result = app::run(
        rx,
        Pipeline::new(pipeline_options(filter, json)),
        view.as_mut(),
        Metrics::new(display.window.unwrap_or(default_window)),
        AppOptions {
            live: true,
            tick: Duration::from_secs(1),
            save: source.save.clone(),
        },
    )
    .await;
    drop(view);
    reader.abort();
    result
}
