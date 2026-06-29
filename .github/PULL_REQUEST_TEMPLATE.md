<!--
One reviewer, one maintainer. Keep this tight — a clear PR gets merged faster.
Security issues do NOT go here: email security@solvela.ai per SECURITY.md.
-->

## What & why

<!-- One or two sentences. Link the issue/PR if there is one (e.g. Closes #123). -->

## Type

- [ ] Bug fix
- [ ] Feature
- [ ] Refactor / cleanup (no behavior change)
- [ ] Docs / config / CI
- [ ] Money path (touches USDC amounts, the 5% fee, cost breakdowns, spend/budget, or x402/escrow verify)

## Checklist

- [ ] `cargo fmt --all -- --check` and `cargo clippy --all-targets --all-features -- -D warnings` pass
- [ ] `cargo test` passes (added/updated tests for the change)
- [ ] `STATUS.md` / `CHANGELOG.md` updated if this ships a user-visible change
- [ ] No secrets in the diff (provider keys, fee-payer key, wallet keys stay in env)
- [ ] DCO `Signed-off-by` trailer present (the `.githooks` hook adds it; CI enforces it)

<!-- Money-path PRs: a second independent review pass is expected before merge. -->
