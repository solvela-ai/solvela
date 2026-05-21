# solvela-client-cli

Agent-side payer CLI for [Solvela](https://solvela.ai). Holds a Solana wallet and
pays gateways for LLM calls with USDC-SPL via the x402 protocol. Installs the
`solvela-client` binary.

```bash
cargo install solvela-client-cli
```

```bash
solvela-client wallet create          # writes ~/.solvela/wallet.json
solvela-client wallet address         # print your public address
solvela-client wallet balance         # USDC-SPL balance
solvela-client chat "explain x402 in one sentence" --model auto
solvela-client models                 # list models the gateway offers
solvela-client doctor                 # check connectivity + configuration
```

The wallet loads from the `SOLVELA_WALLET_KEY` env var (base58 keypair) if set,
otherwise from `~/.solvela/wallet.json`. The gateway defaults to
`https://api.solvela.ai`; override with `-g/--gateway-url`.

> This is the **payer** CLI. The operator/developer CLI for running and
> inspecting a gateway is a different binary, `solvela-cli` (crate
> [`solvela-cli`](https://crates.io/crates/solvela-cli)).

Part of the Solvela Rust client SDK workspace — see the
[workspace README](https://github.com/solvela-ai/solvela/tree/main/sdks/rust).

## License

[MIT](https://github.com/solvela-ai/solvela/blob/main/LICENSE-MIT).
