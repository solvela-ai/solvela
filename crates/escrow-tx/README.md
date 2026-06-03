# solvela-escrow-tx

Pure, dependency-light construction of signed Solana **escrow `deposit`
transactions** for the [Solvela](https://github.com/solvela-ai/solvela) x402
payment protocol.

This crate is the **shared source of truth for the escrow deposit-transaction
byte layout**. It is consumed both by the gateway-side verifier (via
`solvela-x402`, which re-exports it) and by the Rust client SDK
(`solvela-client`), so a client always signs exactly what the gateway and the
on-chain program expect. The byte layout is on-chain-verified and pinned by a
golden-vector test asserted in *both* this crate and `solvela-x402`.

It deliberately has **no `solana-sdk` and no HTTP dependency** — only the crypto
primitives needed to:

- derive the escrow PDA and associated token account (ATA) addresses,
- compute the Anchor instruction discriminator,
- serialize a Solana legacy message, and
- sign it with an ed25519 keypair.

## API

- `deposit::build_deposit_tx(&DepositParams) -> Result<String, DepositError>` —
  builds and signs the escrow `deposit` transaction, returning it
  base64-encoded for the x402 `PAYMENT-SIGNATURE` payload.
- `deposit::GOLDEN_VECTOR_B64` — the pinned reference transaction the
  golden-vector tests assert against.
- `pda` — address helpers: `find_program_address`, `derive_ata_address`,
  `anchor_discriminator`, `decode_bs58_pubkey`, plus the canonical program-id
  constants.

## Status

This is an internal building block of the Solvela SDK stack, published so the
client SDK crates can resolve it from crates.io. Most users should depend on
[`solvela-client`](https://crates.io/crates/solvela-client) rather than using
this crate directly. See the [main repository](https://github.com/solvela-ai/solvela)
for the full payment-gateway and SDK documentation.

## License

Licensed under the [Apache License, Version 2.0](https://www.apache.org/licenses/LICENSE-2.0).
