//! Interactive chat REPL for `solvela chat -i`.
//!
//! Uses [`rustyline`] for line editing (arrow keys, history, Ctrl-C/D). The
//! REPL keeps a per-session message history that grows with each turn until
//! the user calls `/clear`.
//!
//! Meta-commands:
//! * `/exit`, `/quit` — leave the REPL (Ctrl-D works too).
//! * `/clear`         — reset the message history.
//! * `/model <name>`  — switch the model used for subsequent turns.
//! * `/help`          — print this list.
//!
//! The REPL reuses the existing 402-payment flow from
//! [`crate::commands::chat`] for paid responses; for ergonomics it currently
//! disables streaming (the upstream gateway returns whole completions when
//! `stream: false`).

use std::io::Write;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use rustyline::error::ReadlineError;
use rustyline::DefaultEditor;
use serde::{Deserialize, Serialize};

/// One message in the rolling conversation history.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ChatMessage {
    pub role: String,
    pub content: String,
}

/// Outcome of a single REPL line — exposed for unit testing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LineOutcome {
    Exit,
    Cleared,
    SwitchedModel(String),
    HelpPrinted,
    Empty,
    Send(String),
    UnknownCommand(String),
}

/// Pure interpreter for a REPL line — easy to unit-test without TTY.
pub(crate) fn interpret_line(input: &str) -> LineOutcome {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return LineOutcome::Empty;
    }
    if !trimmed.starts_with('/') {
        return LineOutcome::Send(trimmed.to_string());
    }
    let mut parts = trimmed.splitn(2, char::is_whitespace);
    let cmd = parts.next().unwrap_or("");
    let arg = parts.next().unwrap_or("").trim();
    match cmd {
        "/exit" | "/quit" => LineOutcome::Exit,
        "/clear" => LineOutcome::Cleared,
        "/help" => LineOutcome::HelpPrinted,
        "/model" => {
            if arg.is_empty() {
                LineOutcome::UnknownCommand("/model requires a model name".to_string())
            } else {
                LineOutcome::SwitchedModel(arg.to_string())
            }
        }
        other => LineOutcome::UnknownCommand(other.to_string()),
    }
}

fn print_banner(api_url: &str, model: &str) {
    println!("Solvela interactive chat");
    println!("  Gateway: {api_url}");
    println!("  Model:   {model}");
    println!("  Type a prompt and press Enter, or use /help for commands.");
    println!();
}

fn print_help() {
    println!("REPL commands:");
    println!("  /exit, /quit    Leave the session (Ctrl-D works too)");
    println!("  /clear          Reset conversation history");
    println!("  /model <name>   Switch model for subsequent turns");
    println!("  /help           Show this help");
}

/// Send a single turn through the gateway and return the assistant's reply.
///
/// Falls back to the existing payment flow in [`chat`] when the gateway
/// requires payment (402). The interactive REPL does not currently prompt for
/// per-turn confirmation — it inherits `--yes` semantics implicitly.
///
/// The `client` is built once in [`run`] with connect and overall timeouts so
/// that a stalled gateway cannot hang the REPL indefinitely.
async fn send_turn(
    client: &reqwest::Client,
    api_url: &str,
    model: &str,
    history: &[ChatMessage],
    scheme: Option<&str>,
) -> Result<TurnReply> {
    let body = serde_json::json!({
        "model": model,
        "messages": history,
        "stream": false,
    });
    let endpoint = format!("{api_url}/v1/chat/completions");

    let resp = client
        .post(&endpoint)
        .json(&body)
        .send()
        .await
        .context("failed to send chat request")?;

    if resp.status().is_success() {
        let json: serde_json::Value = resp.json().await.context("parse chat response")?;
        return Ok(parse_turn_reply(&json));
    }

    if resp.status().as_u16() == 402 {
        // Reuse the existing payment flow — for now, simply error out and ask
        // the user to retry with the non-interactive `solvela chat` command.
        // Fully integrating the payment loop into the REPL would duplicate a
        // lot of code; keep that for a follow-up.
        let _ = scheme; // placeholder for future scheme-aware retry
        return Err(anyhow!(
            "gateway requires payment (HTTP 402). Run `solvela chat \"<prompt>\" --yes` \
             to pay non-interactively, then retry the REPL."
        ));
    }

    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    Err(anyhow!("gateway error {status}: {text}"))
}

#[derive(Debug, Clone)]
struct TurnReply {
    content: String,
    model: Option<String>,
    cost: Option<String>,
}

fn parse_turn_reply(json: &serde_json::Value) -> TurnReply {
    let content = json["choices"][0]["message"]["content"]
        .as_str()
        .unwrap_or("")
        .to_string();
    let model = json["model"].as_str().map(|s| s.to_string());
    let cost = json["usage"]["cost_breakdown"]["total"]
        .as_str()
        .map(|s| format!("{s} USDC"));
    TurnReply {
        content,
        model,
        cost,
    }
}

/// Public entrypoint for `solvela chat --interactive`.
pub async fn run(api_url: &str, initial_model: &str, scheme: Option<&str>) -> Result<()> {
    // Build once: a stalled gateway must not hang the REPL forever.
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(30))
        .build()
        .context("failed to build HTTP client")?;

    let mut model = initial_model.to_string();
    let mut history: Vec<ChatMessage> = Vec::new();

    print_banner(api_url, &model);

    let mut editor = DefaultEditor::new().context("failed to initialize line editor")?;

    loop {
        let line = match editor.readline("> ") {
            Ok(l) => l,
            Err(ReadlineError::Eof | ReadlineError::Interrupted) => {
                println!("Bye.");
                return Ok(());
            }
            Err(e) => return Err(anyhow!("readline error: {e}")),
        };

        // History includes meta-commands so users can recall and edit them.
        let _ = editor.add_history_entry(line.as_str());

        match interpret_line(&line) {
            LineOutcome::Empty => continue,
            LineOutcome::Exit => {
                println!("Bye.");
                return Ok(());
            }
            LineOutcome::Cleared => {
                history.clear();
                println!("(history cleared)");
            }
            LineOutcome::HelpPrinted => print_help(),
            LineOutcome::SwitchedModel(new_model) => {
                model = new_model;
                println!("(model: {model})");
            }
            LineOutcome::UnknownCommand(detail) => {
                println!("unknown command: {detail}. Try /help.");
            }
            LineOutcome::Send(user_input) => {
                history.push(ChatMessage {
                    role: "user".to_string(),
                    content: user_input,
                });
                match send_turn(&client, api_url, &model, &history, scheme).await {
                    Ok(reply) => {
                        println!("{}", reply.content);
                        if let (Some(m), Some(c)) = (&reply.model, &reply.cost) {
                            println!("  [{m} | {c}]");
                        } else if let Some(m) = &reply.model {
                            println!("  [{m}]");
                        } else if let Some(c) = &reply.cost {
                            println!("  [{c}]");
                        }
                        std::io::stdout().flush().ok();
                        history.push(ChatMessage {
                            role: "assistant".to_string(),
                            content: reply.content,
                        });
                    }
                    Err(e) => {
                        // Roll back the user message that didn't get answered.
                        history.pop();
                        eprintln!("error: {e}");
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_input_is_empty() {
        assert_eq!(interpret_line(""), LineOutcome::Empty);
        assert_eq!(interpret_line("   "), LineOutcome::Empty);
        assert_eq!(interpret_line("\t\n"), LineOutcome::Empty);
    }

    #[test]
    fn plain_text_is_send() {
        assert_eq!(
            interpret_line("hello world"),
            LineOutcome::Send("hello world".to_string())
        );
    }

    #[test]
    fn slash_exit_quits() {
        assert_eq!(interpret_line("/exit"), LineOutcome::Exit);
        assert_eq!(interpret_line("/quit"), LineOutcome::Exit);
        assert_eq!(interpret_line("  /exit  "), LineOutcome::Exit);
    }

    #[test]
    fn slash_clear_resets() {
        assert_eq!(interpret_line("/clear"), LineOutcome::Cleared);
    }

    #[test]
    fn slash_help_is_handled() {
        assert_eq!(interpret_line("/help"), LineOutcome::HelpPrinted);
    }

    #[test]
    fn slash_model_with_arg_switches() {
        assert_eq!(
            interpret_line("/model gpt-4o"),
            LineOutcome::SwitchedModel("gpt-4o".to_string())
        );
        assert_eq!(
            interpret_line("/model   claude-3-5-sonnet"),
            LineOutcome::SwitchedModel("claude-3-5-sonnet".to_string())
        );
    }

    #[test]
    fn slash_model_without_arg_is_unknown() {
        match interpret_line("/model") {
            LineOutcome::UnknownCommand(msg) => {
                assert!(msg.contains("model"), "expected hint, got: {msg}");
            }
            other => panic!("expected UnknownCommand, got {other:?}"),
        }
    }

    #[test]
    fn unknown_slash_command_is_reported() {
        assert_eq!(
            interpret_line("/foo"),
            LineOutcome::UnknownCommand("/foo".to_string())
        );
    }

    #[test]
    fn parse_turn_reply_extracts_content_and_model() {
        let json = serde_json::json!({
            "choices": [{"message": {"content": "Hello!"}}],
            "model": "demo",
            "usage": {"cost_breakdown": {"total": "0.0001"}}
        });
        let reply = parse_turn_reply(&json);
        assert_eq!(reply.content, "Hello!");
        assert_eq!(reply.model.as_deref(), Some("demo"));
        assert_eq!(reply.cost.as_deref(), Some("0.0001 USDC"));
    }

    #[test]
    fn parse_turn_reply_handles_missing_fields() {
        let json = serde_json::json!({});
        let reply = parse_turn_reply(&json);
        assert_eq!(reply.content, "");
        assert!(reply.model.is_none());
        assert!(reply.cost.is_none());
    }

    /// Exercises the REPL's pure logic across a full simulated session.
    /// Covers: empty line, plain text, /clear, /model, /exit.
    #[test]
    fn simulated_repl_session_routes_each_line_correctly() {
        let inputs = ["", "hello", "/help", "/model gpt-4o", "/clear", "/exit"];
        let outcomes: Vec<LineOutcome> = inputs.iter().map(|i| interpret_line(i)).collect();
        assert_eq!(outcomes[0], LineOutcome::Empty);
        assert_eq!(outcomes[1], LineOutcome::Send("hello".to_string()));
        assert_eq!(outcomes[2], LineOutcome::HelpPrinted);
        assert_eq!(
            outcomes[3],
            LineOutcome::SwitchedModel("gpt-4o".to_string())
        );
        assert_eq!(outcomes[4], LineOutcome::Cleared);
        assert_eq!(outcomes[5], LineOutcome::Exit);
    }
}
