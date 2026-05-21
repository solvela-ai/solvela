# solvela-client-cli-args

Shared [clap](https://crates.io/crates/clap) argument structs and wallet-loading
helpers for the [Solvela](https://solvela.ai) payer binaries
(`solvela-client-cli` and `solvela-client-proxy`). Provides `WalletArgs`,
`GatewayArgs`, `RpcArgs`, and a `load_wallet` helper that reads a base58 keypair
from an env var (priority) or a Solana-CLI-format JSON file (fallback).

This is an **internal support crate** for the Solvela Rust SDK binaries. Most
users want [`solvela-client`](https://crates.io/crates/solvela-client) (library)
or [`solvela-client-cli`](https://crates.io/crates/solvela-client-cli) (CLI)
instead.

Part of the Solvela Rust client SDK workspace — see the
[workspace README](https://github.com/solvela-ai/solvela/tree/main/sdks/rust).

## License

[MIT](https://github.com/solvela-ai/solvela/blob/main/LICENSE-MIT).
