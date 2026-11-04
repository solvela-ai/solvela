# Escrow upgrade-authority migration to Squads

Operator procedure for [#175](https://github.com/solvela-ai/solvela/issues/175).
**Status: prepared; mainnet transfer has not been performed by this change.**
The last repository-recorded authority is
`EDM9pao5miQdJYfzCtZii9cVn5ZHTBJq84Y9yyqZbsr4`; check finalized
chain state before proceeding. Do not close #175 until the completion record
below is filled and independently checked.

## Scope and custody

Only the upgrade authority of mainnet escrow program
`9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU` changes. No redeploy,
account migration, IDL change, gateway configuration, or wallet rotation is needed.
The operational payer/recipient is `9QGtTUpvLmhggDuBciAeE67MmhECVFYdFLD7xKD4RSno`;
the issue's reference to `B7reP7…` predates that wallet's retirement.

Phase 1 uses one dedicated member (`EDM9…`), threshold 1. It provides
proposal history and a separate authority address but retains single-key
compromise and loss risk. Keep its signing key off Fly, CI, and the gateway.
Never paste a seed or private key into a website, issue, terminal transcript,
or this repository. Use an operator-controlled signing interface compatible
with Squads; stop if that interface is unavailable.

Squads' [creation guide](https://docs.squads.so/main/getting-started/create-a-squad)
currently lists a 0.1 SOL creation fee plus funding/rent. Check the current UI
quote and [costs](https://docs.squads.so/main/additional-resources/costs-of-using-squads)
before signing; the issue's “free / ~$0.01” estimate is not a funding budget.

## 1. Prepare and rehearse

1. Use [app.squads.so](https://app.squads.so) on mainnet to create a Squads v4
   Squad dedicated to upgrades. Record its multisig address and the specific
   vault PDA/index used by Programs. **The multisig account, vault PDA, and
   member wallet are different addresses.** Only the selected vault PDA becomes
   the program authority.
2. Verify member public keys, permissions (phase 1 needs propose, vote, execute),
   threshold, time lock, and config authority against decoded on-chain Squads
   state. Use a self-governed Squad with no external config authority:
   a separate config key could otherwise override the intended quorum.
   See [permissions](https://docs.squads.so/main/development/cli/commands) and
   [config-authority behavior](https://docs.squads.so/main/development/typescript/instructions/create-multisig).
3. Independently derive the vault PDA from the recorded multisig and vault
   index using the official [Squads v4 SDK](https://github.com/Squads-Protocol/v4),
   and compare it character-for-character with the Programs destination.
   An explorer label or an address copied from chat is insufficient.
4. Rehearse creation, authority transfer, proposal, approval, execution, and
   a subsequent upgrade on a disposable devnet program/Squad. Use a dedicated
   rehearsal key and program; do not transfer mainnet authority to a devnet
   address. Follow the cluster/build-feature rules in
   [the escrow deployment checklist](../../programs/escrow/AGENTS.md).
5. Fund the signing fee payer for creation, proposal rent, and execution.
   Confirm access and backups before handing off authority. Pause concurrent
   program upgrades for the ceremony.

Record public addresses and signatures only. Keep RPC credentials and local
keypair paths out of committed evidence.

## 2. Snapshot mainnet before the handoff

Run in Bash on the operator workstation with Solana CLI installed. Explicit RPC,
commitment, and signer arguments avoid inheriting a devnet/default-wallet setup.

```bash
set -euo pipefail
PROGRAM_ID=9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU
EXPECTED_AUTHORITY=EDM9pao5miQdJYfzCtZii9cVn5ZHTBJq84Y9yyqZbsr4
RPC_URL=https://api.mainnet-beta.solana.com
AUTHORITY_KEYPAIR="$HOME/.config/solana/solvela-upgrade-authority.json"
# Set explicitly to the funded operator-controlled fee-payer signer path/URI.
: "${FEE_PAYER:?Set the fee-payer signer path or URI}"

test "$(solana genesis-hash --url "$RPC_URL")" = \
  5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp
test "$(solana address --keypair "$AUTHORITY_KEYPAIR")" = "$EXPECTED_AUTHORITY"
EVIDENCE_DIR="$(mktemp -d)"
solana --url "$RPC_URL" --commitment finalized program show "$PROGRAM_ID" \
  > "$EVIDENCE_DIR/before.txt"
solana --url "$RPC_URL" --commitment finalized program dump "$PROGRAM_ID" \
  "$EVIDENCE_DIR/before.so"
cat "$EVIDENCE_DIR/before.txt"
sha256sum "$EVIDENCE_DIR/before.so"
```

Stop unless the displayed program ID and Authority exactly match the constants
above. Record ProgramData address, last deploy slot, and bytecode SHA-256.
If chain state differs from repository history, investigate before signing.
Preserve this evidence directory through verification.

## 3. Transfer authority

Preferred: use **Safe Authority Transfer (SAT)** from the Squad's Programs
screen after adding the program ID. Follow the current
[Squads Programs procedure](https://docs.squads.so/main/navigating-your-squad/developers-assets/programs).
Review the destination and proposal instructions before approving/executing.
SAT requires both the current authority and vault to participate, avoiding an
unchecked handoff to an address the Squad cannot control. Use the ceremony's
verified CLI signer; do not add an unrelated hot wallet as a voting member.

CLI fallback, only after the independent vault derivation and devnet rehearsal:

```bash
# Supply the verified MAINNET vault PDA; there is deliberately no default.
: "${SQUADS_VAULT:?Set the independently verified Squads vault PDA}"
solana program set-upgrade-authority --help
solana --url "$RPC_URL" --commitment finalized --fee-payer "$FEE_PAYER" \
  program set-upgrade-authority "$PROGRAM_ID" \
  --upgrade-authority "$AUTHORITY_KEYPAIR" \
  --new-upgrade-authority "$SQUADS_VAULT" \
  --skip-new-upgrade-authority-signer-check
```

The vault is a PDA and cannot sign a standalone CLI transaction as a keypair.
The bypass selects an unchecked authority transfer; it does **not** prove vault
ownership. Verify support in the installed CLI before using it (see
[Agave CLI implementation](https://github.com/anza-xyz/agave/blob/master/cli/src/program.rs)).
Never use `--final`: it irreversibly removes upgradeability.

Save the transaction signature. On timeout or ambiguous CLI output, query
finalized chain state and transaction status before retrying.

## 4. Verify and publish evidence

```bash
solana --url "$RPC_URL" --commitment finalized program show "$PROGRAM_ID" \
  > "$EVIDENCE_DIR/after.txt"
solana --url "$RPC_URL" --commitment finalized program dump "$PROGRAM_ID" \
  "$EVIDENCE_DIR/after.so"
cat "$EVIDENCE_DIR/after.txt"
cmp "$EVIDENCE_DIR/before.so" "$EVIDENCE_DIR/after.so"
sha256sum "$EVIDENCE_DIR/after.so"
```

Require all of the following before declaring success:

- Finalized transfer transaction succeeded; its destination matches the
  independently derived vault PDA.
- Authority is exactly that vault PDA. ProgramData address and last deploy
  slot are unchanged; bytecode comparison passes.
- Squads Programs recognizes the authority. The recorded on-chain members,
  permissions, threshold, config authority, and time lock match the approved
  configuration.
- Devnet rehearsal evidence demonstrates the complete future upgrade flow.

If authority is correct but UI indexing lags, inspect chain state rather than
transferring again. If the destination is wrong, the old single-sig cannot undo
the handoff. Recovery requires the new authority's cooperation; an uncontrolled
destination can permanently strand upgrade rights. For a deliberate reversal
from the correct vault, use a Squads Change authority proposal approved and
executed by the current quorum.

Commit a completed public record here (or link one) and update the current
posture in [SECURITY.md](../../SECURITY.md), [STATUS.md](../../STATUS.md),
[regulatory positioning](../product/regulatory-position.md), and
[feature state](../product/feature-state.md). Preserve dated historical entries.

| Completion evidence | Value |
|---|---|
| Execution date (UTC), operator/reviewer | Pending |
| Mainnet program ID and ProgramData | Pending |
| Squad URL, v4 multisig address | Pending |
| Vault PDA, vault index, independent derivation check | Pending |
| Members/permissions, threshold, config authority, time lock | Pending |
| Creation and transfer signatures, finalized slot | Pending |
| Before/after authority, deploy slot, bytecode SHA-256 | Pending |
| Devnet rehearsal program, Squad, transfer/upgrade signatures | Pending |

## Future upgrades and quorum changes

For upgrades, build/test with matching mainnet features, write a buffer, verify
its bytes against the reviewed artifact, and transfer its authority to the
verified vault. Propose the upgrade in Squads, inspect buffer/program/spill
addresses and source commit, collect approvals, then execute after any time
lock. Verify finalized deployed bytes and that authority remains the vault.
Use the official Programs guide for current commands. The old single-sig alone
can no longer run a direct program upgrade.

Phase 2: add an independent hardware-wallet or trusted co-signer, prove signing
access, then approve/execute a change to 2-of-2. Losing either key blocks all
upgrades; record that recovery limitation. Phase 3: add a separately stored
recovery signer, retain threshold 2, and verify 2-of-3 on-chain. Member/threshold
changes are Squads configuration proposals; the vault address and program
authority need not change. Review pending proposals after configuration changes
and repeat the rehearsal with the new quorum.

