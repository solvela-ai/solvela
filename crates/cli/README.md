# solvela-cli

The `solvela` command-line client for the [Solvela](https://solvela.ai)
LLM payment gateway — wallet, chat, models, escrow, MCP, and load-testing
commands. Talks to the public gateway at <https://api.solvela.ai> over
the [x402 payment protocol](https://x402.org).

## Install

```bash
cargo install solvela-cli
```

## First chat

```bash
# Optional: defaults to https://api.solvela.ai
export SOLVELA_API_URL=https://api.solvela.ai

# Required for any paid command (signs the USDC-SPL transaction).
# Public endpoint is fine for a smoke test; use Helius/QuickNode/Triton
# for real usage to avoid rate limits.
export SOLANA_RPC_URL=https://api.mainnet-beta.solana.com

solvela wallet init                 # generate a Solana keypair (fund the printed address with USDC + a little SOL)
solvela chat --model auto "hello"   # ask the smart router to pick a model
solvela models                      # list available models + per-token pricing
solvela doctor                      # check config + gateway reachability
```

The CLI handles the full x402 handshake — fetch a 402 with cost
breakdown, sign the USDC-SPL payment, retry with the
`payment-signature` header.

## Links

- Gateway: <https://api.solvela.ai>
- Docs: <https://docs.solvela.ai>
- Source: [solvela-ai/solvela `crates/cli/`](https://github.com/solvela-ai/solvela/tree/main/crates/cli)

License: Apache-2.0
