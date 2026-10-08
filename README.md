# rancher

Argument-free bundle entry point for Reiny 0.8. It selects a `main.yaml` deployment
and runs `reiny run <main.yaml> --bin-dir bin`, with Velopack install/update hooks
at the start of `main`. Reiny owns process lifecycle, readiness, reconciliation
and cooperative shutdown; Rancher owns selection and installer integration.

Copyright 2026 nop, MechanicalGirl LLC. Author: nop <noplab90@gmail.com>.
Licensed under Apache-2.0 (see `LICENSE` and `NOTICE`).

## Build

```sh
cargo build --release --locked
cargo install --locked --git https://github.com/MechanicalGirlDev/rancher rancher
```

The package and binary are both named `rancher`. Rename the binary to the bundle name
(for example `<name>.exe`) when staging a bundle.

## Bundle layout

The bundle has the same shape as the source repository. Paths are relative to the executable:

```text
<name>[.exe]                  this launcher, renamed
reiny[.exe]                   bundled reiny (falls back to `reiny` on PATH)
bin/                          --bin-dir; prepended to PATH / LD_LIBRARY_PATH / DYLD_LIBRARY_PATH
default.launch                optional: one line, a bundle-relative launch path or a unique short name
update.url                    optional: one line, Velopack feed URL (no file = no update check)
main.yaml                    optional: the bundle's default module root
projects/<name>/main.yaml     selectable deployments, including their nested module directories
experiments/<id>/projects/<name>/main.yaml   optional experiment projects, same shape
```

Selection rules:

- An argument naming a project (`<name> my_test`) or a bundle-relative path is consumed as the
  selection; all remaining arguments are passed to `reiny run`. Anything else (for example
  `--log-level debug`) is passed through untouched.
- Duplicate short names require the bundle-relative path.
- One entry launches directly. Several show a numbered menu; empty input or EOF
  selects `default.launch`, else the adjacent `main.yaml`, else the first entry.
- `n` duplicates a project directory without changing relative paths. The copied
  root receives the new project name as its deployment identity, preserving any
  explicitly configured domain. Root YAML is reserialized; comments are not
  retained. `.reiny/` owner credentials, process state and caches are not copied.
- `d` deletes a project after a `[y/N]` confirmation (EOF means no), stopping an
  active deployment through Reiny's authenticated owner first. A failed stop
  prevents deletion. The adjacent bundle root itself is not offered for copy or deletion.

Use one `main.yaml` per module directory, explicit port wiring and
immutable Git pins in `lock.yaml`. Managed executables must register their
declared ports before `Cloudy::ready()` and finish cleanup on
`Cloudy::shutdown()`. The old `launch.yaml`/`Reiny.toml` formats and `on_exit`
launcher directives are not supported. See the
[Reiny 0.8 module guide](https://github.com/MechanicalGirlDev/reiny/blob/v0.8.0/docs/modules.md).

## Explicit-launch QA

Run on a scratch bundle directory containing the built binary:

```sh
mkdir -p bundle/projects/a_sim bundle/projects/b_real bundle/bin
printf 'version: 2\ndeployment: a_sim\n' > bundle/projects/a_sim/main.yaml
printf 'version: 2\ndeployment: b_real\n' > bundle/projects/b_real/main.yaml
cp target/release/rancher bundle/rancher
# a fake reiny that prints its arguments
printf '#!/bin/sh\necho "reiny $@"\n' > bundle/reiny && chmod +x bundle/reiny
cd bundle
./rancher b_real --log-level debug   # expect: reiny run .../projects/b_real/main.yaml --bin-dir .../bin --log-level debug
./rancher projects/a_sim/main.yaml   # expect: a_sim selected by bundle-relative path
./rancher </dev/null                 # expect: menu, default (first entry) launched on EOF
```

## Verify

```sh
cargo fmt --all --check
cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
cargo test --locked --workspace --all-features
```

CI also builds the real managed SDK module in `tests/fixtures/module-runtime`
and checks Rancher selection, the reported namespace, readiness, cooperative
stop and native child reaping with Reiny CLI 0.8.0. The standalone, pinned
Sutera differential-drive check separately retains its six-tick motion and
zero-wheel shutdown assertions without making Rancher depend on Sutera's
Reiny adapter.

`tests/fixtures/diffdrive` is a module-root example for the migrated
`sutera-launch` adapter. Supply that 0.8-compatible adapter and its controller/HAL
executables in the bundle's `bin/`. The leaf runs until explicit `reiny stop`
or a foreground shutdown request; it no longer uses the removed
`on_exit: shutdown_all` behavior.

Version-2 leaves own endpoint policies; callers provide wiring rather than
repeating child outputs. The diffdrive leaf declares the controller and HAL as
frozen companions, and its config uses portable `@bin/` names. Config assets are
unneeded for these inline demo settings. For a finite managed run, declare
`run.kind: task` alongside the tick limit; a natural service exit is a failure.
