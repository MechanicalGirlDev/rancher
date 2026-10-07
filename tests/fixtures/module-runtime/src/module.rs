//! A real managed module proving readiness and cooperative stop through the SDK.

use std::path::PathBuf;

fn main() -> reiny::Result<()> {
    let opts = reiny::RuntimeOptions::from_args("rancher-module-smoke");
    let trace = match opts.extra_args.as_slice() {
        [flag, path] if flag == "--trace" => PathBuf::from(path),
        _ => anyhow::bail!("expected --trace <path>"),
    };
    let namespace = opts.id.clone();
    reiny::run_with(opts, move |cloudy| async move {
        cloudy.ready()?;
        cloudy.shutdown().await;
        std::fs::write(trace, namespace)?;
        Ok(())
    })
}
