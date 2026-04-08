# Escrow Mainnet Deployment Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Deploy the Anchor escrow program to Solana mainnet with upgrade authority retained, wire it to the production gateway, and verify end-to-end escrow payment flow.

**Architecture:** The escrow program is already built and tested (21 tests). The gateway already has full escrow integration (EscrowVerifier, EscrowClaimer, claim processor with circuit breaker, fee payer pool rotation). This plan deploys the program, configures the gateway, and runs a live test. No new code needed — just deployment, configuration, and verification.

**Tech Stack:** Anchor CLI 0.31.1, Solana CLI 3.1.12, Fly.io, PostgreSQL (claim queue)

**Prerequisite:** Attorney approval for escrow deployment with upgrade authority retained.

---

## Pre-Deployment Checklist

Before starting any task, verify:
- [ ] Anchor CLI installed: `anchor --version` → 0.31.1
- [ ] Solana CLI installed: `solana --version` → 3.1.12
- [ ] Solana config set to mainnet: `solana config set --url https://api.mainnet-beta.solana.com`
- [ ] Deployer wallet exists: `solana address` → shows an address
- [ ] Deployer wallet funded: `solana balance` → at least 3 SOL (program deployment costs ~2 SOL)

---

### Task 1: Generate Program Keypair

**Context:** The current program ID (`9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU`) is a localnet/devnet placeholder. We need a fresh keypair for mainnet.

- [ ] **Step 1: Generate keypair**

```bash
cd programs/escrow
solana-keygen new --no-bip39-passphrase --outfile mainnet-program-keypair.json
```

Record the public key — this is your mainnet program ID.

- [ ] **Step 2: Update `declare_id!` in lib.rs**

```bash
# In programs/escrow/src/lib.rs, line 44:
# Change:
declare_id!("9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU");
# To:
declare_id!("<YOUR_NEW_PROGRAM_ID>");
```

- [ ] **Step 3: Update Anchor.toml**

Add a `[programs.mainnet-beta]` section and update existing sections:

```toml
[programs.localnet]
rustyclawrouter_escrow = "<YOUR_NEW_PROGRAM_ID>"

[programs.devnet]
rustyclawrouter_escrow = "<YOUR_NEW_PROGRAM_ID>"

[programs.mainnet-beta]
rustyclawrouter_escrow = "<YOUR_NEW_PROGRAM_ID>"
```

- [ ] **Step 4: Verify program still compiles**

```bash
cd programs/escrow
cargo check --lib
cargo test --manifest-path Cargo.toml
```

Expected: all tests pass.

- [ ] **Step 5: Commit**

```bash
git add programs/escrow/src/lib.rs programs/escrow/Anchor.toml
git commit -m "chore: update escrow program ID for mainnet deployment"
```

**DO NOT commit `mainnet-program-keypair.json`** — this is a secret. Store it securely outside the repo.

---

### Task 2: Build and Deploy to Mainnet

**Context:** Anchor build produces a `.so` file (the compiled Solana program). Deployment uploads it to mainnet. Upgrade authority is retained by default — you can revoke later.

- [ ] **Step 1: Build the program**

```bash
cd programs/escrow
anchor build
```

Verify output: `target/deploy/rustyclawrouter_escrow.so` exists.

- [ ] **Step 2: Deploy to mainnet**

```bash
solana program deploy \
  target/deploy/rustyclawrouter_escrow.so \
  --program-id mainnet-program-keypair.json \
  --url https://api.mainnet-beta.solana.com \
  --keypair ~/.config/solana/id.json \
  --commitment finalized \
  --with-compute-unit-price 50000
```

This will take 30-60 seconds. Record the transaction signature.

- [ ] **Step 3: Verify deployment**

```bash
solana program show <YOUR_NEW_PROGRAM_ID> --url mainnet-beta
```

Expected output should show:
- Program Id: your new ID
- Owner: BPFLoaderUpgradeab1e11111111111111111111111
- Authority: your deployer wallet (upgrade authority RETAINED)
- Executable: true

- [ ] **Step 4: Verify upgrade authority is retained**

```bash
solana program show <YOUR_NEW_PROGRAM_ID> --url mainnet-beta | grep Authority
```

Should show your deployer wallet address — NOT "none". This confirms you can still upgrade.

---

### Task 3: Generate and Fund Fee Payer Wallet

**Context:** The escrow claimer needs a fee payer wallet to submit claim transactions on-chain. This wallet pays SOL for transaction fees (~0.005 SOL per claim). It does NOT hold USDC — it only holds SOL.

- [ ] **Step 1: Generate fee payer keypair**

```bash
solana-keygen new --no-bip39-passphrase --outfile fee-payer-keypair.json
```

Record the public key and the base58-encoded full keypair.

- [ ] **Step 2: Extract base58 keypair for Fly.io**

```bash
# The keypair file is a JSON array of 64 bytes. Convert to base58:
python3 -c "
import json, base58
with open('fee-payer-keypair.json') as f:
    key_bytes = bytes(json.load(f))
print(base58.b58encode(key_bytes).decode())
"
```

Save this base58 string — you'll set it as a Fly.io secret.

- [ ] **Step 3: Fund the fee payer wallet**

Transfer SOL from your main wallet:
```bash
solana transfer <FEE_PAYER_ADDRESS> 0.5 --url mainnet-beta
```

0.5 SOL covers ~100 claim transactions. The gateway's balance monitor will warn at 0.1 SOL.

- [ ] **Step 4: Verify balance**

```bash
solana balance <FEE_PAYER_ADDRESS> --url mainnet-beta
```

Expected: 0.5 SOL (or whatever you sent).

**DO NOT commit keypair files** — store them securely outside the repo.

---

### Task 4: Update Fly.io Secrets

**Context:** The gateway reads escrow config from environment variables. Three secrets need to be set (or updated) for escrow to activate.

- [ ] **Step 1: Set escrow program ID**

Via Fly.io dashboard (https://fly.io/apps/rustyclawrouter-gateway/secrets) or CLI:
```bash
fly secrets set RCR_SOLANA__ESCROW_PROGRAM_ID=<YOUR_NEW_PROGRAM_ID> -a rustyclawrouter-gateway
```

- [ ] **Step 2: Set fee payer key**

```bash
fly secrets set RCR_SOLANA__FEE_PAYER_KEY=<BASE58_FEE_PAYER_KEYPAIR> -a rustyclawrouter-gateway
```

- [ ] **Step 3: Verify RPC URL is mainnet**

The RPC URL should already be set from PR #5. Verify:
```bash
fly secrets list -a rustyclawrouter-gateway | grep RPC
```

If it still says devnet, update it:
```bash
fly secrets set RCR_SOLANA__RPC_URL=https://api.mainnet-beta.solana.com -a rustyclawrouter-gateway
```

- [ ] **Step 4: Redeploy gateway**

Setting secrets auto-restarts, but to pick up the latest code:
```bash
cd /home/kennethdixon/projects/RustyClawRouter
git pull origin main
fly deploy -a rustyclawrouter-gateway
```

---

### Task 5: Verify Escrow is Active

**Context:** Once deployed and configured, the gateway should advertise both "exact" and "escrow" payment schemes in 402 responses.

- [ ] **Step 1: Check 402 response includes escrow scheme**

```bash
curl -s -X POST https://rustyclawrouter-gateway.fly.dev/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{"model":"auto","messages":[{"role":"user","content":"test"}]}' \
  | python3 -c "
import sys, json
d = json.load(sys.stdin)
pr = json.loads(d['error']['message'])
for a in pr['accepts']:
    print(f\"scheme: {a['scheme']}, program: {a.get('escrow_program_id', 'n/a')}\")
"
```

Expected:
```
scheme: exact, program: n/a
scheme: escrow, program: <YOUR_NEW_PROGRAM_ID>
```

- [ ] **Step 2: Check escrow health endpoint**

```bash
curl -s https://rustyclawrouter-gateway.fly.dev/v1/escrow/health \
  -H "Authorization: Bearer <ADMIN_TOKEN>" | python3 -m json.tool
```

Should show: escrow claimer configured, fee payer pool active, claim processor running.

- [ ] **Step 3: Check gateway logs for escrow initialization**

```bash
fly logs -a rustyclawrouter-gateway --no-tail | grep -i escrow | head -10
```

Should see "escrow verifier initialized" and "claim processor started" messages.

---

### Task 6: End-to-End Escrow Payment Test

**Context:** This is the real test — send a chat request, pay via escrow, verify the claim is processed.

- [ ] **Step 1: Send a request and pay via escrow**

This requires a client that supports escrow (the CLI currently only supports "exact" scheme). For now, use the Python SDK or construct the request manually:

```python
# Using the Python SDK with escrow support:
from rustyclawrouter import LLMClient

client = LLMClient(
    api_url="https://rustyclawrouter-gateway.fly.dev",
    private_key="<YOUR_AGENT_WALLET_KEYPAIR>",
)
# Note: the SDK currently selects the first accept (exact).
# To test escrow, you need to modify the SDK to prefer escrow,
# or send a manual request with an escrow PaymentPayload.
```

**Alternative:** Test via Telsi (if it's configured to use escrow scheme).

- [ ] **Step 2: Verify the escrow deposit on-chain**

```bash
# Check the escrow PDA account exists after deposit
solana account <PDA_ADDRESS> --url mainnet-beta
```

- [ ] **Step 3: Verify the claim was processed**

Check the claim queue in PostgreSQL:
```bash
fly postgres connect -a rustyclawrouter-db
SELECT * FROM escrow_claim_queue ORDER BY created_at DESC LIMIT 5;
```

Should show a claim entry with `status = 'completed'`.

- [ ] **Step 4: Verify the claim transaction on-chain**

Use the tx signature from the claim queue entry:
```bash
solana confirm <TX_SIGNATURE> --url mainnet-beta -v
```

---

### Task 7: Update Documentation

- [ ] **Step 1: Update HANDOFF.md**

Mark escrow as deployed:
- Escrow program: deployed to mainnet with upgrade authority retained
- Program ID: `<YOUR_NEW_PROGRAM_ID>`
- Deployer wallet: `<ADDRESS>` (holds upgrade authority)
- Fee payer: `<ADDRESS>` (funded with X SOL)

- [ ] **Step 2: Update CHANGELOG.md**

Add entry for escrow mainnet deployment.

- [ ] **Step 3: Update regulatory-position.md**

Change "designed and tested locally" to "deployed to Solana mainnet with upgrade authority retained."

- [ ] **Step 4: Commit**

```bash
git add HANDOFF.md CHANGELOG.md docs/product/regulatory-position.md
git commit -m "docs: escrow program deployed to mainnet"
```

---

## Post-Deployment: Revoking Upgrade Authority (When Ready)

When the attorney gives the green light and you're confident the program is bug-free:

```bash
solana program set-upgrade-authority <YOUR_NEW_PROGRAM_ID> --final --url mainnet-beta
```

**This is irreversible.** After this, nobody can ever modify the program.

Verify:
```bash
solana program show <YOUR_NEW_PROGRAM_ID> --url mainnet-beta | grep Authority
```

Should show: `Authority: none`

---

## Rollback Plan

If something goes wrong after deployment:

1. **Program bug:** You have upgrade authority — deploy a fixed version with `solana program deploy --program-id <ID> <new.so>`
2. **Fee payer out of SOL:** Fund it. The gateway's balance monitor will alert.
3. **Claim processor stuck:** Check `/v1/escrow/health`. The circuit breaker pauses if failure rate > 50% in 5 minutes — investigate the error and it auto-recovers.
4. **Need to disable escrow:** Remove `RCR_SOLANA__ESCROW_PROGRAM_ID` from Fly.io secrets and redeploy. The gateway falls back to exact-only payments.

---

## Execution Order

Tasks 1-4 are sequential (each depends on the previous).
Task 5 can run immediately after Task 4.
Task 6 requires a funded agent wallet with USDC.
Task 7 runs after Task 5 or 6.

**Estimated time:** 30-45 minutes for Tasks 1-5 (assuming funded deployer wallet). Task 6 depends on having a test wallet with mainnet USDC.
