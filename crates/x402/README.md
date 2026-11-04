# solvela-x402

Rust implementation of the [x402 payment protocol](https://x402.org) for
Solana, plus opt-in Nano exact settlement — payment verification, escrow integration, fee-payer pool, and
nonce pool. Powers the [Solvela](https://solvela.ai) LLM gateway.

This crate is HTTP-framework-agnostic. The `PaymentVerifier` trait is
chain-agnostic by design so EVM verifiers can be added later without
disturbing the Solana implementation.

## Install

```toml
[dependencies]
solvela-x402 = "0.2"

# Optional: PostgreSQL-backed nonce store for replay protection.
solvela-x402 = { version = "0.2", features = ["postgres"] }
```

## What's inside

- Solana USDC-SPL `exact`-scheme verification
- Opt-in Nano XNO `exact` settlement through a trusted mainnet node
- Escrow-scheme integration with the Solvela escrow program
- Fee-payer pool — pre-funded keys that sign on behalf of agents
- Nonce pool — durable nonces for offline signing
- `PaymentVerifier` trait — chain-agnostic verifier surface

## Opt-in Nano (XNO) settlement

The gateway can additionally advertise `scheme="exact"`,
`network="nano:mainnet"`, `asset="XNO"` for `/v1/chat/completions` and
`/v1/messages`. Solana USDC-SPL remains the default. Nano is not advertised
for marketplace, search, A2A, escrow, or channel payments.

Configure all three environment variables (or an optional `[nano]` TOML
section with the equivalent field names):

```sh
SOLVELA_NANO__RPC_URL=https://your-trusted-mainnet-node.example
SOLVELA_NANO__RECIPIENT_WALLET=nano_...
SOLVELA_NANO__RAW_PER_USDC_ATOMIC=1000000000000000000000000
```

The example rate means one XNO per USDC; it is an **example, not a market
price or peg**. The operator must set and maintain the rate. The fee-inclusive
micro-USDC quote is multiplied by this integer using checked `u128`
arithmetic. One XNO is 10^30 raw; the challenge's Nano `amount` is a decimal
string in raw units. Existing cost breakdowns, budgets, and spend receipts
remain denominated in USDC-equivalent pricing units; they are not Nano raw
transfer amounts. Overflow/zero quotes are never advertised.

Select the Nano accept from the 402 body. Echo it in `accepted` and encode
a **signed, unpublished state send block JSON** as base64 in
`payload.transaction` (the surrounding payment envelope is the existing
base64 JSON `PAYMENT-SIGNATURE` header). The state block includes `type`,
`account`, `previous`, `representative`, `balance`, `link`, `signature`,
and `work`. Keep the wallet's current representative. Set the signed `link`
to the recipient public key and decrease the balance by at least the
advertised raw amount. Keys and work generation stay client-side.

`NanoVerifier` checks the confirmed predecessor, sender, recipient, and raw
balance delta. Its atomic settlement operation calls synchronous `process`
with `subtype="send"`, then requires a matching `block_info` receipt with
`confirmed="true"`. A successful publish alone is **not** confirmation.
Polling is bounded to ten seconds (or the smaller client timeout).
Standalone `verify_payment` deliberately fails closed: Nano cannot use
Solana's non-mutating verification/deferred-broadcast flow.

**Prepayment semantics:** Nano settles before provider delivery. A provider
failure may leave a confirmed payment requiring operator reconciliation;
there is no automatic refund. The error reports this explicitly and the
collected quote is retained in the spend ledger. An RPC timeout can also
leave a block published without a completion: check the block on-chain and
contact the operator before paying again.

**Replay and node requirements:** all replicas must use the **same trusted,
non-load-balanced mainnet node**. The node's atomic ledger insertion is the
durable replay gate, beyond the gateway's short-lived replay cache. `Old block`,
forks, bad signatures, and bad work are rejected, never treated as idempotent
success. A public hash or previously published send is not a payment credential:
hash-only receipts would allow reuse of public payments after cache expiry.
The gateway does not hold Nano keys or create receive blocks; the operator
receives funds with their own wallet.

See the [Nano RPC protocol](https://docs.nano.org/commands/rpc-protocol/) and
[confirmation tracking guide](https://docs.nano.org/integration-guides/block-confirmation-tracking/).

## Links

- Gateway: <https://api.solvela.ai>
- Docs: <https://docs.solvela.ai>
- Source: [solvela-ai/solvela `crates/x402/`](https://github.com/solvela-ai/solvela/tree/main/crates/x402)

License: Apache-2.0
