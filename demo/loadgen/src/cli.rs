//! Command-line interface.

use std::path::PathBuf;
use std::time::Duration;

use clap::Parser;

use crate::mix::{ReadWrite, parse_read_write};

const LONG_ABOUT: &str = "\
HTTP load generator for the DAT401 multi-Region blog API (Lambda function URLs + Aurora DSQL).

Reads each Region's API URL from demo/outputs.json (or --url), collects real user and post ids \
from the API, then drives a read-heavy blog mix against every Region at once and shows live TPS. \
TPS is successful (2xx) responses per second; every API call reads or writes Aurora DSQL.

Keys on the live display: q / Esc / Ctrl-C stop, + / - raise or lower the target by 25% \
(re-ramped at the --ramp slope).";

const RAMP_LONG_HELP: &str = "\
Seconds to ramp linearly from zero to the target (requests/s, or workers with --concurrency).

The default of 60 s is deliberately gentle:
  * Each new Lambda execution environment opens its own DSQL connection.
  * Each DSQL cluster accepts 100 new connections/s (burst 1,000), up to 10,000 in total.
  * The account's Lambda concurrency is 1,000 per Region today.
Starting at full load forces a storm of cold starts that all open connections at the same \
moment. Ramping lets Lambda add environments a few at a time, which keeps the connection-open \
rate well under the cluster limit and makes the climb visible on screen.

Rule of thumb: new environments/s per Region = (target per Region / ramp) x seconds per request. \
At 20,000 req/s per Region, ~30 ms per request and a 60 s ramp, that is about 10/s.

Use 0 only against a warm deployment or the local mock.";

const CONNECTIONS_LONG_HELP: &str = "\
Maximum connections per Region.

Lambda function URLs only speak HTTP/1.1 (they do not negotiate HTTP/2 over ALPN), so every \
in-flight request needs its own keep-alive connection. This is therefore also the cap on \
in-flight requests per Region. In open-loop mode, requests that would exceed it are skipped and \
counted, which means the generator (or the API) is saturated.

Size it at about target req/s per Region x worst-case latency in seconds, with headroom. If a \
server does negotiate HTTP/2, requests are spread over ceil(N/100) multiplexed connections instead.";

#[derive(Parser, Debug, Clone)]
#[command(
    name = "loadgen",
    version,
    about = "HTTP load generator for the DAT401 multi-Region blog API",
    long_about = LONG_ABOUT
)]
pub struct Args {
    /// demo/outputs.json written by demo/deploy.sh; uses regions[].apiUrl [default: ../outputs.json]
    #[arg(
        long,
        value_name = "PATH",
        help_heading = "Targets",
        conflicts_with = "urls"
    )]
    pub outputs: Option<PathBuf>,

    /// API base URL to use instead of --outputs; repeat once per Region
    #[arg(long = "url", value_name = "URL", help_heading = "Targets")]
    pub urls: Vec<String>,

    /// Split traffic across Regions: "us-east-1=3,us-east-2=1", or "3,1" in target order [default: even]
    #[arg(long, value_name = "WEIGHTS", help_heading = "Targets")]
    pub region_weights: Option<String>,

    /// Open loop: total requests/s across all Regions, sent on schedule whatever the latency [default: 100]
    #[arg(
        long,
        value_name = "N",
        help_heading = "Load",
        conflicts_with = "concurrency"
    )]
    pub rps: Option<f64>,

    /// Closed loop instead: N workers in total, each sending its next request as soon as the last returns
    #[arg(long, value_name = "N", help_heading = "Load")]
    pub concurrency: Option<usize>,

    /// Seconds to ramp linearly from zero to the target; gentle on purpose (see --help for why)
    #[arg(
        long,
        value_name = "SECS",
        default_value_t = 60.0,
        help_heading = "Load",
        long_help = RAMP_LONG_HELP
    )]
    pub ramp: f64,

    /// Stop after this long, e.g. 90s, 10m, 1h30m [default: until Ctrl-C or q]
    #[arg(long, value_name = "DURATION", value_parser = parse_duration, help_heading = "Load")]
    pub duration: Option<Duration>,

    /// Read/write split in percent
    #[arg(
        long,
        value_name = "READ/WRITE",
        default_value = "90/10",
        value_parser = parse_read_write,
        help_heading = "Traffic mix"
    )]
    pub mix: ReadWrite,

    /// Percent of reads that list posts (GET /posts); the rest fetch one post (GET /posts/:id)
    #[arg(
        long,
        value_name = "PCT",
        default_value_t = 40.0,
        help_heading = "Traffic mix"
    )]
    pub list_share: f64,

    /// Percent of list reads that filter by a random category
    #[arg(
        long,
        value_name = "PCT",
        default_value_t = 50.0,
        help_heading = "Traffic mix"
    )]
    pub category_share: f64,

    /// Percent of writes that create a post (POST /posts); the rest comment on a random post.
    /// Off by default: new posts land on the blog's front page, which the talk shows afterwards.
    #[arg(
        long,
        value_name = "PCT",
        default_value_t = 0.0,
        help_heading = "Traffic mix"
    )]
    pub new_post_share: f64,

    /// ?limit= for GET /posts reads
    #[arg(
        long,
        value_name = "N",
        default_value_t = 20,
        help_heading = "Traffic mix"
    )]
    pub page_size: u32,

    /// Post ids to collect before starting (pages through GET /posts with ?before=)
    #[arg(
        long,
        value_name = "N",
        default_value_t = 3000,
        help_heading = "Bootstrap"
    )]
    pub bootstrap_posts: usize,

    /// Re-read the newest posts this often so new posts get read too; 0 turns it off
    #[arg(
        long,
        value_name = "SECS",
        default_value_t = 15.0,
        help_heading = "Bootstrap"
    )]
    pub refresh: f64,

    /// Max connections (= in-flight requests) per Region; see --help
    #[arg(
        long,
        value_name = "N",
        default_value_t = 2048,
        help_heading = "HTTP",
        long_help = CONNECTIONS_LONG_HELP
    )]
    pub connections: usize,

    /// Request timeout in seconds, including reading the body
    #[arg(
        long,
        value_name = "SECS",
        default_value_t = 10.0,
        help_heading = "HTTP"
    )]
    pub timeout: f64,

    /// Connect timeout in seconds (TCP + TLS)
    #[arg(
        long,
        value_name = "SECS",
        default_value_t = 5.0,
        help_heading = "HTTP"
    )]
    pub connect_timeout: f64,

    /// Force HTTP/1.1 [default: negotiate via ALPN; function URLs only offer HTTP/1.1 anyway]
    #[arg(long, help_heading = "HTTP", conflicts_with = "http2_prior_knowledge")]
    pub http1: bool,

    /// Use cleartext HTTP/2 without negotiation (h2c); only useful against the local mock
    #[arg(long, help_heading = "HTTP")]
    pub http2_prior_knowledge: bool,

    /// Tokio worker threads [default: number of CPUs]
    #[arg(long, value_name = "N", help_heading = "HTTP")]
    pub threads: Option<usize>,

    /// Print plain once-per-second lines instead of the full-screen display (automatic when stdout isn't a terminal)
    #[arg(long, help_heading = "Output")]
    pub plain: bool,

    /// Write a JSON report (config, totals, per-Region and per-op stats, per-second timeline) on exit
    #[arg(long, value_name = "FILE", help_heading = "Output")]
    pub report: Option<PathBuf>,

    /// Title for the live display
    #[arg(
        long,
        value_name = "TEXT",
        default_value = "Aurora DSQL · multi-Region blog",
        help_heading = "Output"
    )]
    pub title: String,
}

/// `90` (seconds), `90s`, `500ms`, `10m`, `1h30m`, `1.5m`.
pub fn parse_duration(s: &str) -> Result<Duration, String> {
    let s = s.trim();
    if s.is_empty() {
        return Err("empty duration".into());
    }
    let secs = if let Ok(v) = s.parse::<f64>() {
        v
    } else {
        let mut total = 0.0;
        let mut rest = s;
        while !rest.is_empty() {
            let num_end = rest
                .find(|c: char| !(c.is_ascii_digit() || c == '.'))
                .ok_or_else(|| format!("missing unit at the end of {s:?}"))?;
            if num_end == 0 {
                return Err(format!("expected a number in {s:?}"));
            }
            let value: f64 = rest[..num_end]
                .parse()
                .map_err(|_| format!("bad number in {s:?}"))?;
            rest = &rest[num_end..];
            let unit_end = rest
                .find(|c: char| c.is_ascii_digit() || c == '.')
                .unwrap_or(rest.len());
            let unit = match &rest[..unit_end] {
                "ms" => 0.001,
                "s" | "sec" | "secs" => 1.0,
                "m" | "min" | "mins" => 60.0,
                "h" | "hr" | "hrs" => 3600.0,
                u => return Err(format!("unknown unit {u:?} in {s:?} (use ms, s, m or h)")),
            };
            total += value * unit;
            rest = &rest[unit_end..];
        }
        total
    };
    if !secs.is_finite() || secs < 0.0 {
        return Err(format!("{s:?} is not a valid duration"));
    }
    Ok(Duration::from_secs_f64(secs))
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn parses_durations() {
        assert_eq!(parse_duration("90"), Ok(Duration::from_secs(90)));
        assert_eq!(parse_duration("90s"), Ok(Duration::from_secs(90)));
        assert_eq!(parse_duration("10m"), Ok(Duration::from_secs(600)));
        assert_eq!(parse_duration("1h30m"), Ok(Duration::from_secs(5400)));
        assert_eq!(parse_duration("1.5m"), Ok(Duration::from_secs(90)));
        assert_eq!(parse_duration("250ms"), Ok(Duration::from_millis(250)));
        assert!(parse_duration("").is_err());
        assert!(parse_duration("10x").is_err());
        assert!(parse_duration("m").is_err());
        assert!(parse_duration("-5").is_err());
    }

    #[test]
    fn cli_is_consistent() {
        Args::command().debug_assert();
    }

    #[test]
    fn rps_and_concurrency_conflict() {
        assert!(Args::try_parse_from(["loadgen", "--rps", "10", "--concurrency", "4"]).is_err());
        assert!(Args::try_parse_from(["loadgen", "--outputs", "x", "--url", "http://a"]).is_err());
        let a =
            Args::try_parse_from(["loadgen", "--url", "http://a", "--url", "http://b"]).unwrap();
        assert_eq!(a.urls.len(), 2);
        assert_eq!(a.ramp, 60.0);
        assert_eq!(a.mix.read, 90.0);
    }
}
