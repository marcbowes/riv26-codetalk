//! Where to send load: `demo/outputs.json` or `--url`, and how to split it.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use reqwest::Url;
use serde::Deserialize;

#[derive(Debug, Clone, PartialEq)]
pub struct TargetSpec {
    /// Region name from outputs.json; for --url it comes from /health later.
    pub label: Option<String>,
    /// Base URL ending in '/'.
    pub base: String,
}

#[derive(Deserialize)]
struct Outputs {
    regions: Vec<OutputRegion>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct OutputRegion {
    region: String,
    api_url: String,
}

/// Validate a URL and make sure it ends in '/', so paths can be appended.
pub fn normalize_base(raw: &str) -> Result<String> {
    let u = Url::parse(raw.trim()).with_context(|| format!("invalid URL {raw:?}"))?;
    if !matches!(u.scheme(), "http" | "https") {
        bail!("URL {raw:?} must be http or https");
    }
    if u.host_str().is_none() {
        bail!("URL {raw:?} has no host");
    }
    let mut s = u.to_string();
    if let Some(q) = s.find(['?', '#']) {
        s.truncate(q);
    }
    if !s.ends_with('/') {
        s.push('/');
    }
    Ok(s)
}

pub fn parse_outputs(json: &str) -> Result<Vec<TargetSpec>> {
    let outputs: Outputs =
        serde_json::from_str(json).context("outputs file is not the expected JSON")?;
    if outputs.regions.is_empty() {
        bail!("outputs file lists no regions");
    }
    outputs
        .regions
        .into_iter()
        .map(|r| {
            Ok(TargetSpec {
                base: normalize_base(&r.api_url)
                    .with_context(|| format!("regions[{}].apiUrl", r.region))?,
                label: Some(r.region),
            })
        })
        .collect()
}

pub fn from_outputs(path: &Path) -> Result<Vec<TargetSpec>> {
    let json =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    parse_outputs(&json).with_context(|| path.display().to_string())
}

pub fn from_urls(urls: &[String]) -> Result<Vec<TargetSpec>> {
    urls.iter()
        .map(|u| {
            Ok(TargetSpec {
                label: None,
                base: normalize_base(u)?,
            })
        })
        .collect()
}

/// Places to look for outputs.json when --outputs isn't given: relative to
/// the working directory (running from demo/loadgen), then next to this
/// crate's source (running the binary from anywhere on the build machine).
pub fn default_outputs_candidates() -> Vec<PathBuf> {
    vec![
        PathBuf::from("../outputs.json"),
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../outputs.json"),
    ]
}

pub fn host_label(base: &str) -> String {
    Url::parse(base)
        .ok()
        .and_then(|u| {
            let host = u.host_str()?.to_string();
            Some(match u.port() {
                Some(p) => format!("{host}:{p}"),
                None => host,
            })
        })
        .unwrap_or_else(|| base.to_string())
}

/// `--region-weights`: either `name=w,name=w` or positional `w,w` (also `3:1`).
pub fn parse_weights(spec: &str, labels: &[String]) -> Result<Vec<f64>> {
    let parts: Vec<&str> = spec
        .split([',', ':'])
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    let weight = |s: &str| -> Result<f64> {
        let w: f64 = s
            .parse()
            .with_context(|| format!("weight {s:?} is not a number"))?;
        if !w.is_finite() || w < 0.0 {
            bail!("weight {s:?} must be zero or positive");
        }
        Ok(w)
    };
    let weights = if parts.iter().any(|p| p.contains('=')) {
        let mut w = vec![0.0; labels.len()];
        for p in &parts {
            let (name, v) = p
                .split_once('=')
                .with_context(|| format!("mixing named and positional weights in {spec:?}"))?;
            let i = labels
                .iter()
                .position(|l| l == name.trim())
                .with_context(|| {
                    format!(
                        "no target named {:?} (have: {})",
                        name.trim(),
                        labels.join(", ")
                    )
                })?;
            w[i] = weight(v.trim())?;
        }
        w
    } else {
        if parts.len() != labels.len() {
            bail!(
                "{} weights given for {} targets ({})",
                parts.len(),
                labels.len(),
                labels.join(", ")
            );
        }
        parts.iter().map(|p| weight(p)).collect::<Result<_>>()?
    };
    if weights.iter().sum::<f64>() <= 0.0 {
        bail!("at least one Region weight must be positive");
    }
    Ok(weights)
}

/// Split a total by weight.
pub fn split(total: f64, weights: &[f64]) -> Vec<f64> {
    let sum: f64 = weights.iter().sum();
    weights.iter().map(|w| total * w / sum).collect()
}

/// Split whole workers by weight (largest remainder), giving every target with
/// a positive weight at least one worker when there are enough to go around.
pub fn split_workers(total: usize, weights: &[f64]) -> Vec<usize> {
    let exact = split(total as f64, weights);
    let mut out: Vec<usize> = exact.iter().map(|x| x.floor() as usize).collect();
    let mut left = total - out.iter().sum::<usize>();
    let mut order: Vec<usize> = (0..weights.len()).collect();
    order.sort_by(|a, b| {
        let ra = exact[*a] - exact[*a].floor();
        let rb = exact[*b] - exact[*b].floor();
        rb.total_cmp(&ra)
    });
    for i in order {
        if left == 0 {
            break;
        }
        if weights[i] > 0.0 {
            out[i] += 1;
            left -= 1;
        }
    }
    // Starved targets borrow from the largest.
    for i in 0..out.len() {
        if weights[i] > 0.0 && out[i] == 0 {
            let (j, max) = out
                .iter()
                .enumerate()
                .max_by_key(|(_, n)| **n)
                .map(|(j, n)| (j, *n))
                .unwrap();
            if max > 1 {
                out[j] -= 1;
                out[i] = 1;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{
      "witnessRegion": "us-west-2",
      "regions": [
        { "region": "us-east-1", "apiUrl": "https://abc.lambda-url.us-east-1.on.aws/", "clusterId": "x",
          "clusterArn": "arn", "clusterEndpoint": "x.dsql.us-east-1.on.aws", "functionName": "f" },
        { "region": "us-east-2", "apiUrl": "https://def.lambda-url.us-east-2.on.aws", "clusterId": "y",
          "clusterArn": "arn", "clusterEndpoint": "y.dsql.us-east-2.on.aws", "functionName": "g" }
      ],
      "webUrl": "https://dxxxx.cloudfront.net",
      "events": { "region": "us-east-1", "streamName": "dat401-cdc" }
    }"#;

    #[test]
    fn reads_outputs_json() {
        let t = parse_outputs(SAMPLE).unwrap();
        assert_eq!(
            t,
            vec![
                TargetSpec {
                    label: Some("us-east-1".into()),
                    base: "https://abc.lambda-url.us-east-1.on.aws/".into()
                },
                TargetSpec {
                    label: Some("us-east-2".into()),
                    base: "https://def.lambda-url.us-east-2.on.aws/".into()
                },
            ]
        );
        assert!(parse_outputs(r#"{"regions":[]}"#).is_err());
        assert!(parse_outputs(r#"{"regions":[{"region":"a","apiUrl":"ftp://x"}]}"#).is_err());
    }

    #[test]
    fn normalizes_urls() {
        assert_eq!(
            normalize_base("http://127.0.0.1:8080").unwrap(),
            "http://127.0.0.1:8080/"
        );
        assert_eq!(normalize_base("https://h/api").unwrap(), "https://h/api/");
        assert_eq!(
            normalize_base("https://h/api/?x=1").unwrap(),
            "https://h/api/"
        );
        assert!(normalize_base("not a url").is_err());
        assert_eq!(host_label("http://127.0.0.1:8080/"), "127.0.0.1:8080");
        assert_eq!(
            host_label("https://abc.lambda-url.us-east-1.on.aws/"),
            "abc.lambda-url.us-east-1.on.aws"
        );
    }

    #[test]
    fn parses_weights() {
        let labels = vec!["us-east-1".to_string(), "us-east-2".to_string()];
        assert_eq!(parse_weights("3,1", &labels).unwrap(), vec![3.0, 1.0]);
        assert_eq!(parse_weights("3:1", &labels).unwrap(), vec![3.0, 1.0]);
        assert_eq!(
            parse_weights("us-east-2=1, us-east-1=4", &labels).unwrap(),
            vec![4.0, 1.0]
        );
        assert_eq!(
            parse_weights("us-east-1=1", &labels).unwrap(),
            vec![1.0, 0.0]
        );
        assert!(parse_weights("1", &labels).is_err());
        assert!(parse_weights("0,0", &labels).is_err());
        assert!(parse_weights("us-west-2=1", &labels).is_err());
        assert!(parse_weights("-1,2", &labels).is_err());
        assert!(parse_weights("us-east-1=1,2", &labels).is_err());
    }

    #[test]
    fn splits_rates_and_workers() {
        assert_eq!(split(1000.0, &[3.0, 1.0]), vec![750.0, 250.0]);
        assert_eq!(split_workers(10, &[1.0, 1.0]), vec![5, 5]);
        assert_eq!(split_workers(10, &[3.0, 1.0]), vec![8, 2]);
        assert_eq!(split_workers(7, &[1.0, 1.0, 1.0]).iter().sum::<usize>(), 7);
        assert_eq!(split_workers(4, &[100.0, 1.0]), vec![3, 1]);
        assert_eq!(split_workers(4, &[1.0, 0.0]), vec![4, 0]);
    }
}
