//! x402 payment protocol — chain-agnostic core plus optional Solana modules.
//!
//! The `traits`, `types`, and `facilitator` modules are always available and
//! contain no chain-specific code. Solana verification, escrow, fee payer
//! pool, nonce pool, and SPL transfer parsing live behind the `solana`
//! cargo feature (enabled by default).
//!
//! # Features
//!
//! - `solana` (default): on-chain Solana verification, escrow, fee payer pool,
//!   nonce pool. Pulls in `bs58`, `sha2`, `ed25519-dalek`, `curve25519-dalek`,
//!   `zeroize`, `reqwest`, and `metrics`.
//! - `postgres`: durable escrow claim queue backed by PostgreSQL via `sqlx`.
//!   Implies `solana`.
//!
//! Disable default features to compile a chain-agnostic core only:
//!
//! ```toml
//! solvela-x402 = { version = "0.1", default-features = false }
//! ```

pub mod facilitator;
pub mod traits;
pub mod types;

#[cfg(feature = "solana")]
pub mod escrow;
#[cfg(feature = "solana")]
pub mod fee_payer;
#[cfg(feature = "solana")]
pub mod nonce_pool;
#[cfg(feature = "solana")]
pub mod solana;
#[cfg(feature = "solana")]
pub mod solana_rpc;
#[cfg(feature = "solana")]
pub mod solana_types;
#[cfg(feature = "solana")]
pub(crate) mod spl_transfer;
