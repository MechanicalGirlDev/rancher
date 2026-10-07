//! Exercise the actual Rancher, Reiny CLI, owner and managed SDK module.

use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use anyhow::{Context, Result, ensure};
use reiny_launch::{DeploymentClient, DeploymentPhase, DeploymentStatus};
use tokio::io::{AsyncBufReadExt, BufReader};

#[tokio::main]
async fn main() -> Result<()> {
    let bundle = PathBuf::from(std::env::args_os().nth(1).context("expected bundle path")?)
        .canonicalize()?;
    let root = bundle.join("projects/managed");
    let trace = root.join("stopped.txt");
    if trace.exists() {
        std::fs::remove_file(&trace)?;
    }
    let namespace = "rancher-module-qa/worker";
    let mut child = tokio::process::Command::new(
        bundle.join(format!("rancher{}", std::env::consts::EXE_SUFFIX)),
    )
    .args(["managed", "--detach"])
    .stdin(Stdio::null())
    .stdout(Stdio::piped())
    .stderr(Stdio::inherit())
    .kill_on_drop(true)
    .spawn()?;
    // The pipe captures the readiness event before the child can publish it.
    let mut lines = BufReader::new(child.stdout.take().context("launcher stdout")?).lines();
    let ready = tokio::time::timeout(Duration::from_secs(60), async {
        let mut message = String::new();
        loop {
            let line = lines
                .next_line()
                .await?
                .context("launcher exited before readiness")?;
            if message.is_empty() && !line.trim_start().starts_with('{') {
                println!("{line}");
                continue;
            }
            message.push_str(&line);
            message.push('\n');
            match serde_json::from_str::<DeploymentStatus>(&message) {
                Ok(state) => return Ok::<_, anyhow::Error>(state),
                Err(error) if error.is_eof() => {}
                Err(error) => return Err(error.into()),
            }
        }
    })
    .await
    .context("launcher readiness deadline")
    .and_then(|state| state);

    // Stop through the authenticated owner even if an assertion would fail.
    let client = DeploymentClient::find(&root)?.context("managed owner missing")?;
    let stopped = client.stop()?;
    let exit = tokio::time::timeout(Duration::from_secs(10), child.wait())
        .await
        .context("launcher exit deadline")??;
    let ready = ready?;
    ensure!(exit.success(), "Rancher failed: {exit}");
    ensure!(ready.phase == DeploymentPhase::Ready && ready.owner_alive);
    let module = ready
        .modules
        .get(namespace)
        .context("namespaced module missing")?;
    let pid = module.pid.context("ready module has no PID")?;
    ensure!(
        module
            .report
            .as_ref()
            .context("module contract report missing")?
            .namespace
            == namespace,
        "module namespace was not preserved"
    );
    ensure!(stopped.phase == DeploymentPhase::Stopped && !stopped.owner_alive);
    ensure!(stopped.modules.values().all(|module| module.pid.is_none()));
    ensure!(std::fs::read_to_string(trace)? == namespace);

    let present = if cfg!(windows) {
        let output = std::process::Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/FO", "CSV", "/NH"])
            .output()?;
        ensure!(output.status.success(), "native process query failed");
        let expected = format!("\"{pid}\"");
        output
            .stdout
            .split(|&byte| byte == b'\n')
            .filter_map(|line| line.split(|&byte| byte == b',').nth(1))
            .any(|field| field == expected.as_bytes())
    } else {
        let output = std::process::Command::new("ps")
            .args(["-p", &pid.to_string(), "-o", "pid="])
            .output()?;
        ensure!(
            matches!(output.status.code(), Some(0 | 1)),
            "native process query failed"
        );
        !output.stdout.is_empty()
    };
    ensure!(!present, "managed child remains after stop");
    println!("Managed Rancher/Reiny readiness, namespace and cooperative cleanup passed");
    Ok(())
}
