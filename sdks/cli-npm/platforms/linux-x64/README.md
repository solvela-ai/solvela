# @solvela/cli-linux-x64

Linux x64 native binary for the Solvela CLI.

This package is an **optional dependency** of `@solvela/cli`. Install the
meta-package instead:

```bash
npm install -g @solvela/cli
```

## Binary

The `bin/solvela` file is **not committed to git**. It is produced by the
`cargo-dist` release workflow (`.github/workflows/release.yml`) and uploaded
to npm during the Phase 4 publish step.

The `bin/` directory contains only a `.gitkeep` placeholder in source control.
See `docs/runbooks/phase-4-publish.md` for the publish runbook.

## Platform

- OS: Linux
- CPU: x64 (glibc)
- Built from: `crates/cli/` — `cargo build --release -p solvela-cli --target x86_64-unknown-linux-gnu`
