use std::io::{self, IsTerminal, Write};
use std::pin::pin;

use futures::StreamExt;

use solvela_client::{ClientBuilder, SolvelaClient};
use solvela_client_cli_args::{load_wallet, GatewayArgs, RpcArgs, WalletArgs};
use solvela_protocol::{ChatMessage, ChatRequest, Role};

pub async fn run(
    prompt: &str,
    model: &str,
    no_stream: bool,
    max_payment: Option<u64>,
    wallet_args: &WalletArgs,
    gateway_args: &GatewayArgs,
    rpc_args: &RpcArgs,
) -> Result<(), String> {
    let wallet = load_wallet(wallet_args)?;

    let mut builder = ClientBuilder::new()
        .gateway_url(&gateway_args.gateway)
        .rpc_url(&rpc_args.rpc_url);

    if let Some(max) = max_payment {
        builder = builder.max_payment_amount(max);
    }

    let config = builder.build_config();

    let client =
        SolvelaClient::new(wallet, config).map_err(|e| format!("failed to create client: {e}"))?;

    let req = ChatRequest {
        model: model.to_string(),
        messages: vec![ChatMessage {
            role: Role::User,
            content: prompt.to_string(),
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

    let is_tty = io::stdout().is_terminal();

    if is_tty && !no_stream {
        // Streaming mode
        let stream = client
            .chat_stream(req)
            .await
            .map_err(|e| format!("stream error: {e}"))?;
        let mut stream = pin!(stream);

        let mut model_name = String::new();
        while let Some(result) = stream.next().await {
            let chunk = result.map_err(|e| format!("stream chunk error: {e}"))?;
            if model_name.is_empty() {
                model_name.clone_from(&chunk.model);
            }
            for choice in &chunk.choices {
                if let Some(ref content) = choice.delta.content {
                    print!("{content}");
                    io::stdout()
                        .flush()
                        .map_err(|e| format!("flush error: {e}"))?;
                }
            }
        }
        println!();

        if !model_name.is_empty() {
            eprintln!("[model: {model_name}]");
        }
    } else if is_tty {
        // Non-streaming TTY mode
        let resp = client
            .chat(req)
            .await
            .map_err(|e| format!("chat error: {e}"))?;

        if let Some(choice) = resp.choices.first() {
            println!("{}", choice.message.content);
        }

        eprintln!("[model: {}]", resp.model);
        if let Some(ref usage) = resp.usage {
            eprintln!(
                "[tokens: {} prompt + {} completion = {} total]",
                usage.prompt_tokens, usage.completion_tokens, usage.total_tokens
            );
        }
    } else {
        // Piped / non-TTY mode — output full JSON
        let resp = client
            .chat(req)
            .await
            .map_err(|e| format!("chat error: {e}"))?;

        let output = serde_json::to_string_pretty(&resp)
            .map_err(|e| format!("failed to serialize response: {e}"))?;
        println!("{output}");
    }

    Ok(())
}
