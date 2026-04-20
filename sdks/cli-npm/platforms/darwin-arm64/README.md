# @solvela/cli-darwin-arm64

macOS ARM64 (Apple Silicon) native binary for the Solvela CLI.

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

- OS: macOS
- CPU: arm64 (Apple Silicon — M1/M2/M3/M4)
- Built from: `crates/cli/` — `cargo build --release -p solvela-cli --target aarch64-apple-darwin`
