//! `--run-on-ec2` and `--stop-ec2`: run this same command on the load
//! generator host (demo/infra `Dat401-Loadgen`) instead of locally.
//!
//! The host has no inbound ports; we reach it with SSM Session Manager
//! (`aws ssm start-session` with `AWS-StartInteractiveCommand`), which gives the
//! remote process a terminal, so the live display shows here while the load
//! comes from EC2 next to the API. It uses the local `aws` CLI and
//! session-manager-plugin, so AWS_PROFILE and friends apply as usual.
//!
//! Each run: start the host if it's stopped, wait for SSM, then on the host
//! wait for the first-boot build, `git pull` + incremental build, copy this
//! machine's outputs.json across, and exec loadgen with the same arguments.

use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use base64::Engine as _;
use serde::Deserialize;

use crate::targets::default_outputs_candidates;

#[derive(Deserialize)]
struct Outputs {
    loadgen: Option<Host>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Host {
    region: String,
    instance_id: String,
}

/// Flags that only make sense on this machine: dropped from the forwarded command.
const LOCAL_FLAGS: &[&str] = &["--run-on-ec2", "--stop-ec2"];

fn outputs_path(explicit: Option<&Path>) -> Result<PathBuf> {
    if let Some(p) = explicit {
        return Ok(p.to_path_buf());
    }
    default_outputs_candidates()
        .into_iter()
        .find(|p| p.is_file())
        .context("no outputs.json found (run demo/deploy.sh, or pass --outputs)")
}

fn host(outputs_json: &str) -> Result<Host> {
    let o: Outputs = serde_json::from_str(outputs_json).context("outputs.json is not the expected JSON")?;
    o.loadgen
        .context("outputs.json has no `loadgen` host (deploy it with demo/deploy.sh; LOADGEN_HOST=0 skips it)")
}

/// Run the local AWS CLI and return trimmed stdout.
fn aws(region: &str, args: &[&str]) -> Result<String> {
    let out = Command::new("aws")
        .args(args)
        .args(["--region", region, "--output", "text"])
        .output()
        .context("running the aws CLI (is it installed and on PATH?)")?;
    if !out.status.success() {
        bail!(
            "aws {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn instance_state(h: &Host) -> Result<String> {
    aws(
        &h.region,
        &[
            "ec2",
            "describe-instances",
            "--instance-ids",
            &h.instance_id,
            "--query",
            "Reservations[0].Instances[0].State.Name",
        ],
    )
}

fn ensure_running(h: &Host) -> Result<()> {
    let id = h.instance_id.as_str();
    let state = instance_state(h)?;
    match state.as_str() {
        "running" => {}
        "pending" => {}
        "stopped" | "stopping" => {
            if state == "stopping" {
                eprintln!("{id} is stopping; waiting before starting it again...");
                aws(&h.region, &["ec2", "wait", "instance-stopped", "--instance-ids", id])?;
            }
            eprintln!("Starting {id}...");
            aws(&h.region, &["ec2", "start-instances", "--instance-ids", id])?;
        }
        other => bail!("load generator host {id} is {other}"),
    }
    aws(&h.region, &["ec2", "wait", "instance-running", "--instance-ids", id])?;

    // The SSM agent takes a little while to check in after a boot.
    let deadline = Instant::now() + Duration::from_secs(300);
    let filter = format!("Key=InstanceIds,Values={id}");
    loop {
        let ping = aws(
            &h.region,
            &[
                "ssm",
                "describe-instance-information",
                "--filters",
                &filter,
                "--query",
                "InstanceInformationList[0].PingStatus",
            ],
        )?;
        if ping == "Online" {
            return Ok(());
        }
        if Instant::now() > deadline {
            bail!("{id} is running but SSM hasn't come online (ping status {ping})");
        }
        eprintln!("Waiting for SSM on {id} ({ping})...");
        std::thread::sleep(Duration::from_secs(5));
    }
}

/// POSIX shell single-quoting.
pub fn sh_quote(s: &str) -> String {
    if !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_./=:,+@%".contains(c))
    {
        return s.to_string();
    }
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// This invocation's arguments, minus the ones that only apply locally.
/// `--outputs` is dropped too: the host gets a copy of the file at its default path.
pub fn forwarded_args(argv: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    let mut it = argv.iter();
    while let Some(a) = it.next() {
        if LOCAL_FLAGS.contains(&a.as_str()) || a.starts_with("--outputs=") {
            continue;
        }
        if a == "--outputs" {
            it.next();
            continue;
        }
        out.push(a.clone());
    }
    out
}

/// The script run as ec2-user on the host.
pub fn remote_script(outputs_b64: Option<&str>, args: &[String]) -> String {
    let mut s = String::from(
        r#"set -e
cd "$HOME/riv26-codetalk"
if [ ! -f "$HOME/READY" ]; then
  echo "First-boot setup is still running (Rust + build). Waiting..."
  while [ ! -f "$HOME/READY" ]; do sleep 10; tail -n 1 /var/log/dat401-setup.log 2>/dev/null || true; done
fi
source "$HOME/.cargo/env"
echo "Updating to origin/main..."
if git pull --ff-only -q; then
  cargo build --release -q --manifest-path demo/loadgen/Cargo.toml || echo "Build failed; using the previous binary."
else
  echo "git pull failed; using the existing checkout."
fi
"#,
    );
    if let Some(b64) = outputs_b64 {
        s.push_str(&format!("echo {b64} | base64 -d > demo/outputs.json\n"));
    }
    s.push_str("cd demo/loadgen\nexec ./target/release/loadgen");
    for a in args {
        s.push(' ');
        s.push_str(&sh_quote(a));
    }
    s.push('\n');
    s
}

/// Replace this process with an SSM session running loadgen on the host.
pub fn run_on_ec2(outputs: Option<&Path>, have_urls: bool) -> Result<()> {
    let path = outputs_path(outputs)?;
    let json = std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let h = host(&json).with_context(|| path.display().to_string())?;

    if Command::new("session-manager-plugin").arg("--version").output().is_err() {
        bail!(
            "session-manager-plugin isn't installed; see \
             https://docs.aws.amazon.com/systems-manager/latest/userguide/session-manager-working-with-install-plugin.html"
        );
    }
    ensure_running(&h)?;

    let argv: Vec<String> = std::env::args().skip(1).collect();
    // With --url the targets travel on the command line; otherwise send our outputs.json.
    let b64 = (!have_urls).then(|| base64::engine::general_purpose::STANDARD.encode(json.as_bytes()));
    let script = remote_script(b64.as_deref(), &forwarded_args(&argv));
    let command = format!("sudo -u ec2-user -H bash -lc {}", sh_quote(&script));
    let params = serde_json::json!({ "command": [command] }).to_string();

    eprintln!(
        "Running on {} ({}) via SSM. Stop the host afterwards with: loadgen --stop-ec2",
        h.instance_id, h.region
    );
    let err = Command::new("aws")
        .args(["ssm", "start-session", "--region", &h.region, "--target", &h.instance_id])
        .args(["--document-name", "AWS-StartInteractiveCommand", "--parameters", &params])
        .exec();
    Err(err).context("starting the SSM session")
}

pub fn stop_ec2(outputs: Option<&Path>) -> Result<()> {
    let path = outputs_path(outputs)?;
    let json = std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let h = host(&json).with_context(|| path.display().to_string())?;
    let state = instance_state(&h)?;
    if state == "stopped" || state == "stopping" {
        eprintln!("{} is already {state}.", h.instance_id);
        return Ok(());
    }
    aws(&h.region, &["ec2", "stop-instances", "--instance-ids", &h.instance_id])?;
    eprintln!("Stopping {} ({}). `--run-on-ec2` starts it again.", h.instance_id, h.region);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(xs: &[&str]) -> Vec<String> {
        xs.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn quoting() {
        assert_eq!(sh_quote("--rps"), "--rps");
        assert_eq!(sh_quote("20000"), "20000");
        assert_eq!(sh_quote("DAT401 finale"), "'DAT401 finale'");
        assert_eq!(sh_quote("it's"), r"'it'\''s'");
        assert_eq!(sh_quote(""), "''");
    }

    #[test]
    fn forwards_everything_but_local_flags() {
        assert_eq!(
            forwarded_args(&v(&["--run-on-ec2", "--rps", "20000", "--outputs", "../x.json", "--title", "Hi there"])),
            v(&["--rps", "20000", "--title", "Hi there"])
        );
        assert_eq!(
            forwarded_args(&v(&["--outputs=../x.json", "--plain", "--run-on-ec2"])),
            v(&["--plain"])
        );
    }

    #[test]
    fn script_copies_outputs_and_execs() {
        let s = remote_script(Some("e30="), &v(&["--rps", "100", "--title", "A b"]));
        assert!(s.contains("echo e30= | base64 -d > demo/outputs.json"));
        assert!(s.ends_with("exec ./target/release/loadgen --rps 100 --title 'A b'\n"));
        assert!(!remote_script(None, &[]).contains("outputs.json"));
    }

    #[test]
    fn parses_host() {
        let h = host(r#"{"regions": [], "loadgen": {"region": "us-west-2", "instanceId": "i-123"}}"#).unwrap();
        assert_eq!((h.region.as_str(), h.instance_id.as_str()), ("us-west-2", "i-123"));
        assert!(host(r#"{"regions": []}"#).is_err());
    }
}
