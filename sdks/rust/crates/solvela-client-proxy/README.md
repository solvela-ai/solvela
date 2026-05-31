# solvela-client-proxy

Localhost reverse proxy with transparent x402 USDC-SPL payment handling for
[Solvela](https://solvela.ai). Point any OpenAI-compatible client at the proxy
and it signs and pays Solana payments on every request — no client code changes.
Installs the `solvela-client-proxy` binary.

```bash
cargo install solvela-client-proxy
solvela-client-proxy --port 8402 --max-payment 0.10
# then set your OpenAI client's base URL to http://localhost:8402
```

Flags:

- `-p/--port` — port to listen on (binds `127.0.0.1` only; default `8402`)
- `--max-payment` — maximum payment per request in USDC (e.g. `0.10`)
- `--expected-recipient` — reject payments to any gateway recipient but this one
- `-g/--gateway-url` — upstream gateway (default `https://api.solvela.ai`)
- `--wallet-env` / `--wallet-file` — keypair source (env var, then file)

The wallet loads from the `SOLVELA_WALLET_KEY` env var (base58 keypair) if set,
otherwise from `~/.solvela/wallet.json`.

Part of the Solvela Rust client SDK workspace — see the
[workspace README](https://github.com/solvela-ai/solvela/tree/main/sdks/rust).

## License

[Apache-2.0](https://github.com/solvela-ai/solvela/blob/main/LICENSE-APACHE).
