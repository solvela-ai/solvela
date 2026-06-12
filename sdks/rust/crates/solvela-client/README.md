# solvela-client

Library for paying [Solvela](https://solvela.ai) gateways with USDC-SPL on Solana
via the x402 protocol. Provides `Wallet` (keypair management), `SolvelaClient`
(the x402 payment handshake), and the request/response types re-exported from
`solvela-protocol`.

```bash
cargo add solvela-client solvela-protocol
```

```rust
use solvela_client::{ClientBuilder, SolvelaClient, Wallet};
use solvela_protocol::{ChatMessage, ChatRequest, Role};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let wallet = Wallet::from_env("SOLVELA_WALLET_KEY")?;
    let config = ClientBuilder::new()
        .gateway_url("https://api.solvela.ai")
        .build_config();
    let client = SolvelaClient::new(wallet, config)?;

    let req = ChatRequest {
        model: "auto".to_string(),
        messages: vec![ChatMessage {
            role: Role::User,
            content: "hello".to_string(),
            name: None,
            tool_calls: None,
            tool_call_id: None,
        }],
        max_tokens: None,
        temperature: None,
        top_p: None,
        stream: false,
        tools: None,
        tool_choice: None,
    };

    // Performs the full x402 handshake: send → sign the 402 challenge → retry.
    let resp = client.chat(req).await?;
    println!("{}", resp.choices[0].message.content);
    Ok(())
}
```

`SolvelaClient::chat_stream` runs the same flow as an SSE stream. If the gateway
returns 402 *again* after a signed payment was attached, both `chat` and
`chat_stream` return `ClientError::PaymentRejected` carrying the rejection body —
distinguishing "signed and rejected" from a pre-signing 402 challenge on both
paths (same semantics as the Python and Go SDKs). `ClientBuilder` exposes escrow
preference, a max-payment cap, response caching, sessions, quality retries, and
a free-fallback model.

Part of the Solvela Rust client SDK workspace. For the CLI, the localhost proxy,
and the full picture see the
[workspace README](https://github.com/solvela-ai/solvela/tree/main/sdks/rust).

## License

[Apache-2.0](https://github.com/solvela-ai/solvela/blob/main/LICENSE-APACHE).
