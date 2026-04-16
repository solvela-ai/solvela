<!-- Parent: ../AGENTS.md -->
<!-- Generated: 2026-04-16 | Updated: 2026-04-16 -->

# src

## Purpose
Implementation of the x402 payment protocol over Solana. Splits into: chain-agnostic traits/types, Solana-specific verification (SPL TransferChecked / Transfer), facilitator RPC client, fee-payer pool, nonce pool, and the escrow integration.

## Key Files
| File | Description |
|------|-------------|
| `lib.rs` | Module declarations — see module list below |
| `traits.rs` | `PaymentVerifier` trait (chain-agnostic seam) |
| `types.rs` | Core protocol types: `PaymentRequired`, `PaymentPayload`, `PaymentScheme`, settlement result |
| `solana.rs` | Solana payment verification — decodes TransferChecked / Transfer, checks program id, destination ATA, amount, mint |
| `solana_types.rs` | Solana-specific message / signature / instruction parsers |
| `solana_rpc.rs` | Thin RPC client over `reqwest` (`getBalance`, `simulateTransaction`, `sendTransaction`) |
| `spl_transfer.rs` | (crate-private) SPL-token instruction layout helpers |
| `facilitator.rs` | Facilitator HTTP client — off-chain verifier/settler endpoints |
| `fee_payer.rs` | Fee-payer keypair pool; rotates keys, tracks balances |
| `nonce_pool.rs` | Durable-nonce account pool for replay-safe unsigned transactions |

## Subdirectories
| Directory | Purpose |
|-----------|---------|
| `escrow/` | Trustless USDC-SPL escrow client: deposit / claim / refund + PDA derivation + claim queue (see `escrow/AGENTS.md`) |

## For AI Agents

### Working In This Directory
- Any function that touches private-key bytes must consume a `zeroize::Zeroizing<_>` or equivalent — never plain `Vec<u8>` of secret material.
- Solana constants (USDC mint, TOKEN_PROGRAM_ID, TOKEN_2022_PROGRAM_ID) live here — don't duplicate them into the gateway.
- `PaymentVerifier` is the seam — keep it chain-agnostic. Solana details stay in `solana.rs` / `solana_types.rs`.
- Replay protection is the caller's responsibility (gateway uses Redis); this crate only verifies the signed transaction is valid.

### Testing Requirements
```bash
cargo test -p x402                    # all tests
cargo test -p x402 solana             # pattern match
```

### Common Patterns
- u64 atomic USDC (6 decimals); no f64 anywhere.
- `anyhow` forbidden — use `thiserror` error enums.
- RPC retries with exponential backoff inside `solana_rpc.rs`.

## Dependencies

### Internal
- `solvela-protocol` for shared types.

### External
- `ed25519-dalek`, `curve25519-dalek`, `zeroize`, `sha2`, `base64`, `bs58`, `reqwest`, `tokio`, `tracing`, `metrics`, `sqlx` (optional).

<!-- MANUAL: -->
