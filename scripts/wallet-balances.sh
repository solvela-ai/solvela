#!/usr/bin/env bash
#
# Solvela Operator Wallet Balances
#
# Snapshots SOL + USDC balances for every operator wallet in one shot so you
# don't have to look each one up on the explorer. Reads addresses from the
# local keypair files in ~/.config/solana/ and the gateway recipient from .env.
#
# Usage:
#   ./scripts/wallet-balances.sh                    # mainnet-beta (default)
#   SOLANA_RPC=https://api.devnet.solana.com ./scripts/wallet-balances.sh
#   RECIPIENT=<pubkey> ./scripts/wallet-balances.sh # explicit override
#
# Environment:
#   SOLANA_RPC   RPC endpoint (default: https://api.mainnet-beta.solana.com)
#   USDC_MINT    USDC mint (default: mainnet EPjF…Dt1v; devnet 4zMM…ncDU)
#   RECIPIENT    Gateway recipient pubkey; overrides the .env lookup

set -euo pipefail

SOLANA_RPC="${SOLANA_RPC:-https://api.mainnet-beta.solana.com}"
USDC_MINT="${USDC_MINT:-EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v}"

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ENV_FILE="$REPO_ROOT/.env"
SOLANA_DIR="$HOME/.config/solana"

if ! command -v solana >/dev/null || ! command -v spl-token >/dev/null; then
  echo "error: requires solana + spl-token CLIs on PATH" >&2
  exit 1
fi

# Resolve recipient wallet: explicit env var wins, otherwise scan .env for any
# key ending in RECIPIENT_WALLET (covers both the nested SOLVELA_SOLANA__…
# form and the legacy single-underscore form).
recipient="${RECIPIENT:-}"
if [[ -z "$recipient" && -f "$ENV_FILE" ]]; then
  recipient="$(
    grep -E '^[A-Z_]*RECIPIENT_WALLET=' "$ENV_FILE" \
      | tail -n1 | cut -d= -f2- | tr -d '"' | tr -d "'" | xargs || true
  )"
fi

# (label, source) pairs — keypair files resolved with `solana address -k`,
# recipient is already an address.
declare -a ROWS=()
add_keypair() {
  local label="$1" path="$2"
  if [[ -f "$path" ]]; then
    local addr
    addr="$(solana address -k "$path" 2>/dev/null || true)"
    [[ -n "$addr" ]] && ROWS+=("$label|$addr")
  fi
}
add_keypair "operator / payer (id.json)"      "$SOLANA_DIR/id.json"
add_keypair "ops (solvela-ops.json)"          "$SOLANA_DIR/solvela-ops.json"
add_keypair "upgrade authority"               "$SOLANA_DIR/solvela-upgrade-authority.json"
[[ -n "$recipient" ]] && ROWS+=("gateway recipient (.env)|$recipient")

if [[ ${#ROWS[@]} -eq 0 ]]; then
  echo "no wallets resolved — check ~/.config/solana/ and .env" >&2
  exit 1
fi

# Print SOL + USDC for each address. USDC is fetched via JSON-RPC
# `getTokenAccountsByOwner` so it works regardless of spl-token CLI flag
# evolution and returns 0 cleanly when the owner has no ATA for the mint.
fetch_sol() {
  solana balance "$1" --url "$SOLANA_RPC" 2>/dev/null || echo "err"
}

fetch_usdc() {
  local owner="$1"
  local resp
  resp="$(curl -s -X POST -H 'Content-Type: application/json' \
    -d "$(printf '{"jsonrpc":"2.0","id":1,"method":"getTokenAccountsByOwner","params":["%s",{"mint":"%s"},{"encoding":"jsonParsed"}]}' "$owner" "$USDC_MINT")" \
    "$SOLANA_RPC" 2>/dev/null || true)"
  # uiAmountString sums across all token accounts (usually one ATA). If the
  # owner has no token account for this mint, the value array is empty and
  # we want to report "0" rather than an empty string.
  local total
  total="$(printf '%s' "$resp" \
    | grep -oE '"uiAmountString":"[0-9.]+"' \
    | sed -E 's/.*"([0-9.]+)".*/\1/' \
    | awk 'BEGIN{s=0} {s+=$1} END{printf "%.6f", s}')"
  if [[ -z "$total" || "$total" == "0.000000" ]]; then
    echo "0"
  else
    echo "$total"
  fi
}

printf "%-38s  %-44s  %16s  %16s\n" "WALLET" "ADDRESS" "SOL" "USDC"
printf "%-38s  %-44s  %16s  %16s\n" "------" "-------" "---" "----"
for row in "${ROWS[@]}"; do
  label="${row%%|*}"
  addr="${row#*|}"
  sol="$(fetch_sol "$addr")"
  usdc="$(fetch_usdc "$addr")"
  printf "%-38s  %-44s  %16s  %16s\n" "$label" "$addr" "${sol:-?}" "${usdc:-0}"
done

echo
echo "rpc:  $SOLANA_RPC"
echo "mint: $USDC_MINT"
