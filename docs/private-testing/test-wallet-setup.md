# Creating a Test Wallet for Solvela

Never use a personal wallet with real assets for testing. Follow this guide to create a secure throwaway wallet.

## Step 1: Generate a keypair

```bash
# macOS / Linux
mkdir -p ~/.solvela
solana-keygen new --outfile ~/.solvela/test-keypair.json

# Windows (if solana-cli is installed)
mkdir %USERPROFILE%\.solvela
solana-keygen new --outfile %USERPROFILE%\.solvela\test-keypair.json
```

Press Enter twice (no passphrase) for a test wallet.

Output:
```
Generating a new keypair
For added security, enter a BIP39 passphrase [hint 'none']:
Wrote new keypair to /Users/you/.solvela/test-keypair.json
========================================================================================
pubkey: DKK5uK8oHfDfLqbLMjR6HvGj5N3HdKJPQCsb5u8wKZ6e
========================================================================================
Save this seed phrase and your BIP39 passphrase to recover your new keypair:
[12-word seed phrase appears here]
```

**Save the pubkey.** You'll need it in the next step.

## Step 2: Extract the base58 secret key

The installer or MCP server needs `SOLANA_WALLET_KEY` as a base58-encoded secret key, not the JSON file.

```bash
# Extract secret array from the JSON keypair
solana-keygen show ~/.solvela/test-keypair.json --outfile /tmp/sk.txt

# Read it
cat /tmp/sk.txt
```

This prints an array like `[144, 67, 201, ...]` — that's NOT what we need. Instead:

```bash
# Simpler: use the --format base58 flag (solana-cli >= 1.14)
solana-keygen show ~/.solvela/test-keypair.json --format base58-secret

# Output: a single base58 string, ~88 characters
# Example: 2pXsyUb5gvr...{88 chars total}...Zq
```

Copy that string. This is your `SOLANA_WALLET_KEY`.

**Test it:**
```bash
export SOLANA_WALLET_KEY="<paste-your-base58-string>"
echo $SOLANA_WALLET_KEY
# Should be ~88 characters, no spaces
```

## Step 3: Fund the wallet with ~$5 USDC

You need USDC on Solana Mainnet. Two options:

### Option A: Withdraw USDC from a CEX (easiest)

1. Log into a CEX (Coinbase, Kraken, OKX, etc.)
2. Withdraw ~$5 USDC to Mainnet (NOT Devnet!)
3. Paste the pubkey from Step 1 as the destination
4. Confirm the withdrawal (takes 2–10 minutes)
5. Verify on [Solscan](https://solscan.io/):
   ```
   https://solscan.io/account/DKK5uK8oHfDfLqbLMjR6HvGj5N3HdKJPQCsb5u8wKZ6e
   ```
   Replace the pubkey with yours. You should see ~$5 USDC SPL token balance.

### Option B: Swap SOL → USDC on Jupiter (if you have SOL)

1. Go to [Jupiter.ag](https://jupiter.ag)
2. Connect your wallet
3. Swap ~$5 SOL → USDC
4. Confirm the transaction
5. Done

## Step 4: Verify the wallet

```bash
# Check the balance using the Solvela CLI (after install)
solvela wallet status --address <your-pubkey>

# Or check manually on Solscan
# https://solscan.io/account/<your-pubkey>
# Look for "Token Balances" → "USDC"
```

You should see a balance like:
```
USDC: $4.99 (11,999,999 tokens at 2 decimals = $119,999.99 / 1,000,000)
SOL: 0.005 (for rent + fees)
```

## Step 5: Set the environment variable

Store the base58 secret key where the MCP server can find it.

### Option A: Export in your shell profile (macOS/Linux)

```bash
# Add to ~/.bashrc, ~/.zshrc, or equivalent
export SOLANA_WALLET_KEY="<your-base58-key>"
export SOLANA_WALLET_ADDRESS="<your-pubkey>"
export SOLANA_RPC_URL="https://api.mainnet-beta.solana.com"
```

Then reload:
```bash
source ~/.bashrc
echo $SOLANA_WALLET_KEY  # Verify it's set
```

### Option B: Store in ~/.solvela/env (Cursor users recommended)

```bash
mkdir -p ~/.solvela
cat > ~/.solvela/env << 'EOF'
SOLANA_WALLET_KEY=<your-base58-key>
SOLANA_WALLET_ADDRESS=<your-pubkey>
SOLANA_RPC_URL=https://api.mainnet-beta.solana.com
EOF

chmod 0600 ~/.solvela/env
```

The Cursor MCP installer writes `"envFile": "${userHome}/.solvela/env"` to your config by default — Cursor will source this file automatically.

### Option C: Windows via environment variables UI

1. Open "Edit environment variables" (Settings → Environment Variables)
2. Add three user variables:
   - `SOLANA_WALLET_KEY` = `<your-base58-key>`
   - `SOLANA_WALLET_ADDRESS` = `<your-pubkey>`
   - `SOLANA_RPC_URL` = `https://api.mainnet-beta.solana.com`
3. Click OK, restart your terminal
4. Verify:
   ```
   echo %SOLANA_WALLET_KEY%
   ```

## Step 6: Secure the keypair file

```bash
# macOS / Linux — restrict to you only
chmod 0600 ~/.solvela/test-keypair.json

# Windows — right-click file → Properties → Security → Edit
# Remove all users except your account, grant Full Control to yourself
```

**Never commit this file to git.** Add to `.gitignore`:
```
.solvela/
*.env
test-keypair.json
```

## Monitoring spend

Throughout testing, check your wallet balance:

```bash
# Via Solvela CLI
solvela wallet status --address <your-pubkey>

# Via Solscan
# https://solscan.io/account/<your-pubkey>

# Via Solana CLI
solana --url https://api.mainnet-beta.solana.com balance <your-pubkey>
```

Expected testing spend: **$0.01–$0.10** over 2 weeks (varies by how much you chat).

## Cleanup after testing

```bash
# Delete the keypair file
rm ~/.solvela/test-keypair.json

# Clear env vars (optional)
unset SOLANA_WALLET_KEY
unset SOLANA_WALLET_ADDRESS

# Optional: transfer any remaining balance to a safe address
# (We don't recommend keeping test wallets long-term)

# Check that the wallet is empty on Solscan
```

## Troubleshooting

| Problem | Fix |
|---------|-----|
| "Invalid keypair" from CLI | Ensure `--outfile` path exists. Try `mkdir -p ~/.solvela` first. |
| Can't extract base58 secret | Update solana-cli: `cargo install solana-cli --force`. Or use `solana-keygen show --format base58-secret`. |
| Wallet not funded after 10 min | CEX withdrawal may be slow. Check the CEX transaction status. Verify the pubkey is exactly correct (copy-paste, no edits). |
| "Address not found on-chain" from Solscan | Mainnet is correct. Fund to the public key (e.g. `DKK5uK8...`), not a private key. |
| Windows: "is not recognized as an internal or external command" | `solana-cli` not in PATH. Reinstall via `scoop` or add to PATH manually. |

## RPC URL

The MCP server uses `SOLANA_RPC_URL` to verify signatures and check wallet balance.

**Recommended (free):**
- `https://api.mainnet-beta.solana.com` (Solana Foundation official)

**Alternatives:**
- Helius RPC (requires free API key): `https://mainnet.helius-rpc.com/?api-key=...`
- QuickNode (requires account): `https://solana-mainnet.quiknode.pro/...`

Stick with the foundation RPC for testing unless you hit rate limits.

---

You're ready! Proceed to `tester-guide.md` to start testing.
