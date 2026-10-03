# rancher

Argument-free bundle entry point. It selects a launch config beside the executable and runs
`reiny run <launch.yaml> --bin-dir bin`, with Velopack install/update hooks at the start of `main`.

Copyright 2026 nop, MechanicalGirl LLC. Author: nop <noplab90@gmail.com>.
Licensed under Apache-2.0 (see `LICENSE` and `NOTICE`). Extracted from
MechanicalGirlDev/humanoid-system.

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
projects/<name>/launch.yaml   launchable projects (fixed file name, relative paths based on its directory)
experiments/<id>/projects/<name>/launch.yaml   optional experiment projects, same shape
```

Selection rules:

- An argument naming a project (`<name> my_test`) or a bundle-relative path is consumed as the
  selection; all remaining arguments are passed to `reiny run`. Anything else (for example
  `--log-level debug`) is passed through untouched.
- Duplicate short names require the bundle-relative path.
- One project launches directly. Several show a numbered menu; empty input or EOF selects the
  default (`default.launch`, else the first entry). `n` duplicates a project directory and `d`
  deletes one after a `[y/N]` confirmation (EOF means no).

## Explicit-launch QA

Run on a scratch bundle directory containing the built binary:

```sh
mkdir -p bundle/projects/a_sim bundle/projects/b_real bundle/bin
printf 'launch:\n' > bundle/projects/a_sim/launch.yaml
printf 'launch:\n' > bundle/projects/b_real/launch.yaml
cp target/release/rancher bundle/rancher
# a fake reiny that prints its arguments
printf '#!/bin/sh\necho "reiny $@"\n' > bundle/reiny && chmod +x bundle/reiny
cd bundle
./rancher b_real --log-level debug   # expect: reiny run .../projects/b_real/launch.yaml --bin-dir .../bin --log-level debug
./rancher projects/a_sim/launch.yaml # expect: a_sim selected by bundle-relative path
./rancher </dev/null                 # expect: menu, default (first entry) launched on EOF
```

## Verify

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
```
