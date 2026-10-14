use std::path::PathBuf;
use std::time::Duration;

use clap::{Args, Parser, Subcommand};

use crate::config::{StartPos, parse_duration};
use crate::style::ColorChoice;

#[derive(Parser, Debug)]
#[command(name = "events", version)]
#[command(
    about = "Tail Aurora DSQL CDC records from a Kinesis data stream, with lag broken down by stage"
)]
#[command(args_conflicts_with_subcommands = true)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,
    /// With no subcommand, `events` runs `tail` with these options.
    #[command(flatten)]
    pub tail: TailArgs,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// One line per change, with per-stage lag (the default)
    Tail(TailArgs),
    /// Refreshing summary by table/op with lag percentiles, for heavy load
    Stats(StatsArgs),
    /// Feed a JSON-lines file of CDC records through the same pipeline, without AWS
    Replay(ReplayArgs),
}

#[derive(Args, Debug)]
pub struct TailArgs {
    #[command(flatten)]
    pub source: SourceArgs,
    #[command(flatten)]
    pub filter: FilterArgs,
    /// Print each reassembled CDC record as JSON instead of a summary line
    #[arg(long)]
    pub json: bool,
    #[command(flatten)]
    pub display: DisplayArgs,
}

#[derive(Args, Debug)]
pub struct StatsArgs {
    #[command(flatten)]
    pub source: SourceArgs,
    #[command(flatten)]
    pub filter: FilterArgs,
    #[command(flatten)]
    pub display: DisplayArgs,
}

#[derive(Args, Debug)]
pub struct ReplayArgs {
    /// JSON lines: bare CDC records, or {"arrival","received","shard","data"} wrappers (see README)
    pub file: PathBuf,
    /// Playback speed relative to the recorded timing; 0 means as fast as possible
    #[arg(long, default_value_t = 1.0)]
    pub speed: f64,
    /// Longest pause between two records
    #[arg(long, default_value = "2s", value_parser = parse_duration)]
    pub max_gap: Duration,
    /// Show the stats view instead of one line per change
    #[arg(long, conflicts_with = "json")]
    pub stats: bool,
    /// Print each reassembled CDC record as JSON
    #[arg(long)]
    pub json: bool,
    #[command(flatten)]
    pub filter: FilterArgs,
    #[command(flatten)]
    pub display: DisplayArgs,
}

#[derive(Args, Debug)]
pub struct SourceArgs {
    /// Kinesis stream name or ARN (takes precedence over --outputs)
    #[arg(long, short = 's')]
    pub stream: Option<String>,
    /// AWS Region [default: from the ARN or outputs.json, then the usual AWS env/profile chain]
    #[arg(long)]
    pub region: Option<String>,
    /// AWS profile to use instead of the default chain
    #[arg(long)]
    pub profile: Option<String>,
    /// outputs.json written by demo/deploy.sh [default: ../demo/outputs.json if it exists]
    #[arg(long)]
    pub outputs: Option<PathBuf>,
    /// Use the outputs.json `demoCdc` stream (the polished app's cluster) instead of `events`
    #[arg(long)]
    pub demo_cluster: bool,
    /// Where to start: latest, trim-horizon, a lookback like 5m or 1h (AT_TIMESTAMP), or an RFC 3339 time
    #[arg(long, default_value = "latest", value_parser = StartPos::parse)]
    pub from: StartPos,
    /// GetRecords interval per shard, in ms (Kinesis allows 5 reads/s per shard)
    #[arg(long, default_value_t = 250, value_parser = clap::value_parser!(u64).range(50..))]
    pub poll_ms: u64,
    /// How often to re-list shards to pick up resharding
    #[arg(long, default_value = "10s", value_parser = parse_duration)]
    pub relist: Duration,
    /// Also append every raw Kinesis record to this JSON-lines file, for `replay` later
    #[arg(long)]
    pub save: Option<PathBuf>,
}

#[derive(Args, Debug)]
pub struct FilterArgs {
    /// Only show these tables (repeat or comma-separate; `table` or `schema.table`)
    #[arg(long = "table", short = 't', value_delimiter = ',')]
    pub tables: Vec<String>,
    /// Give up on a chunked record if its fragments haven't all arrived within this long
    #[arg(long, default_value = "2m", value_parser = parse_duration)]
    pub chunk_ttl: Duration,
}

#[derive(Args, Debug)]
pub struct DisplayArgs {
    #[arg(long, value_enum, default_value_t = ColorChoice::Auto)]
    pub color: ColorChoice,
    /// Window for lag percentiles, in seconds [default: 60 for tail, 10 for stats]
    #[arg(long, value_parser = clap::value_parser!(u64).range(1..=3600))]
    pub window: Option<u64>,
    /// No status line (tail)
    #[arg(long, short = 'q')]
    pub quiet: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn cli_is_consistent() {
        Cli::command().debug_assert();
    }

    #[test]
    fn default_subcommand_is_tail() {
        let cli = Cli::try_parse_from([
            "events",
            "--stream",
            "s",
            "--table",
            "comments,posts",
            "-t",
            "users",
        ])
        .unwrap();
        assert!(cli.command.is_none());
        assert_eq!(cli.tail.source.stream.as_deref(), Some("s"));
        assert_eq!(cli.tail.filter.tables, ["comments", "posts", "users"]);
        assert_eq!(cli.tail.source.from, StartPos::Latest);
        assert_eq!(cli.tail.source.poll_ms, 250);
    }

    #[test]
    fn subcommands() {
        let cli = Cli::try_parse_from([
            "events",
            "stats",
            "--demo-cluster",
            "--from",
            "trim-horizon",
        ])
        .unwrap();
        let Some(Command::Stats(s)) = cli.command else {
            panic!()
        };
        assert!(s.source.demo_cluster);
        assert_eq!(s.source.from, StartPos::TrimHorizon);

        let cli = Cli::try_parse_from(["events", "replay", "samples/blog.jsonl", "--speed", "0"])
            .unwrap();
        let Some(Command::Replay(r)) = cli.command else {
            panic!()
        };
        assert_eq!(r.speed, 0.0);
        assert_eq!(r.max_gap, Duration::from_secs(2));

        assert!(Cli::try_parse_from(["events", "replay", "f", "--stats", "--json"]).is_err());
        assert!(Cli::try_parse_from(["events", "--poll-ms", "10"]).is_err());
        assert!(Cli::try_parse_from(["events", "--from", "soon"]).is_err());
    }
}
