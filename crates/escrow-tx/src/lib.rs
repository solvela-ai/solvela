//! Solvela escrow transaction builder.
//!
//! Pure, dependency-light construction of signed Solana escrow `deposit`
//! transactions for client SDKs. No `solana-sdk`, no HTTP client — only the
//! crypto primitives needed to derive PDAs/ATAs, compute the Anchor
//! instruction discriminator, serialize a Solana legacy message, and sign it
//! with an ed25519 keypair.
//!
//! This crate is the shared source of truth for escrow deposit-tx byte layout.
//! It is consumed by `solvela-x402` (which re-exports it) and by the Rust
//! client SDK (`solvela-client`). The byte layout is on-chain-verified and is
//! pinned by a golden-vector test in both this crate and `solvela-x402`.

pub mod deposit;
pub mod pda;
