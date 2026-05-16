use std::collections::HashSet;

use solvela_client::{ClientBuilder, SolvelaClient, Wallet};
use solvela_client_cli_args::{load_wallet, GatewayArgs, RpcArgs, WalletArgs};
use solvela_protocol::PaymentRequired;

pub async fn run(
    wallet_args: &WalletArgs,
    gateway_args: &GatewayArgs,
    rpc_args: &RpcArgs,
) -> Result<(), String> {
    let http = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| format!("failed to build HTTP client: {e}"))?;

    let gateway_url = gateway_args.gateway.trim_end_matches('/');

    // 1. Wallet loaded
    let wallet = check_wallet(wallet_args);
    let wallet_ok = wallet.is_some();

    // 2. Gateway reachable
    let gateway_ok = check_gateway(&http, gateway_url).await;

    // 3. Models available
    check_models(&http, gateway_url, gateway_ok).await;

    // 4. Solana RPC reachable
    let rpc_ok = check_rpc(&http, &rpc_args.rpc_url).await;

    // 5. USDC balance
    check_balance(wallet, wallet_ok, rpc_ok, gateway_url, &rpc_args.rpc_url).await;

    // 6. Payment flow
    check_payment_flow(&http, gateway_url, gateway_ok).await;

    Ok(())
}

fn check_wallet(wallet_args: &WalletArgs) -> Option<Wallet> {
    match load_wallet(wallet_args) {
        Ok(w) => {
            println!("[ok]  Wallet loaded: {}", w.address());
            Some(w)
        }
        Err(e) => {
            println!("[FAIL] Wallet: {e}");
            None
        }
    }
}

async fn check_gateway(http: &reqwest::Client, gateway_url: &str) -> bool {
    match http.get(format!("{gateway_url}/health")).send().await {
        Ok(resp) if resp.status().is_success() => {
            println!("[ok]  Gateway reachable: {gateway_url}");
            true
        }
        Ok(resp) => {
            println!(
                "[FAIL] Gateway returned HTTP {}: {gateway_url}",
                resp.status()
            );
            false
        }
        Err(e) => {
            println!("[FAIL] Gateway unreachable: {e}");
            false
        }
    }
}

async fn check_models(http: &reqwest::Client, gateway_url: &str, gateway_ok: bool) {
    if !gateway_ok {
        println!("[--]  Models: skipped (gateway unreachable)");
        return;
    }

    match http.get(format!("{gateway_url}/v1/models")).send().await {
        Ok(resp) if resp.status().is_success() => {
            let body = resp.text().await.unwrap_or_default();
            match serde_json::from_str::<Vec<solvela_protocol::ModelInfo>>(&body) {
                Ok(models) => {
                    let providers: HashSet<&str> =
                        models.iter().map(|m| m.provider.as_str()).collect();
                    println!(
                        "[ok]  Models available: {} model(s) from {} provider(s)",
                        models.len(),
                        providers.len()
                    );
                }
                Err(e) => {
                    println!("[FAIL] Models: failed to parse response: {e}");
                }
            }
        }
        Ok(resp) => {
            println!("[FAIL] Models endpoint returned HTTP {}", resp.status());
        }
        Err(e) => {
            println!("[FAIL] Models: {e}");
        }
    }
}

async fn check_rpc(http: &reqwest::Client, rpc_url: &str) -> bool {
    let rpc_body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "getSlot",
        "params": []
    });

    match http.post(rpc_url).json(&rpc_body).send().await {
        Ok(resp) if resp.status().is_success() => {
            let json: serde_json::Value = resp.json().await.unwrap_or_default();
            if let Some(slot) = json.get("result").and_then(serde_json::Value::as_u64) {
                println!("[ok]  Solana RPC reachable: slot {slot}");
                true
            } else if json.get("error").is_some() {
                println!("[FAIL] Solana RPC error: {json}");
                false
            } else {
                println!("[WARN] Solana RPC reachable but unexpected response");
                true
            }
        }
        Ok(resp) => {
            println!("[FAIL] Solana RPC returned HTTP {}", resp.status());
            false
        }
        Err(e) => {
            println!("[FAIL] Solana RPC unreachable: {e}");
            false
        }
    }
}

async fn check_balance(
    wallet: Option<Wallet>,
    wallet_ok: bool,
    rpc_ok: bool,
    gateway_url: &str,
    rpc_url: &str,
) {
    if !wallet_ok || !rpc_ok {
        let reason = if wallet_ok {
            "RPC unreachable"
        } else {
            "wallet not loaded"
        };
        println!("[--]  USDC balance: skipped ({reason})");
        return;
    }

    let Some(w) = wallet else {
        return;
    };

    let config = ClientBuilder::new()
        .gateway_url(gateway_url)
        .rpc_url(rpc_url)
        .build_config();

    match SolvelaClient::new(w, config) {
        Ok(client) => match client.usdc_balance().await {
            Ok(balance) => {
                if balance < 0.01 {
                    println!("[WARN] USDC balance: {balance:.6} (low balance, may not be enough for requests)");
                } else {
                    println!("[ok]  USDC balance: {balance:.6}");
                }
            }
            Err(e) => {
                println!("[FAIL] USDC balance: {e}");
            }
        },
        Err(e) => {
            println!("[FAIL] USDC balance: failed to create client: {e}");
        }
    }
}

async fn check_payment_flow(http: &reqwest::Client, gateway_url: &str, gateway_ok: bool) {
    if !gateway_ok {
        println!("[--]  Payment flow: skipped (gateway unreachable)");
        return;
    }

    let probe = serde_json::json!({
        "model": "auto",
        "messages": [{"role": "user", "content": "doctor probe"}],
        "stream": false
    });

    match http
        .post(format!("{gateway_url}/v1/chat/completions"))
        .json(&probe)
        .send()
        .await
    {
        Ok(resp) if resp.status().as_u16() == 402 => {
            let body = resp.text().await.unwrap_or_default();
            match serde_json::from_str::<PaymentRequired>(&body) {
                Ok(pr) => {
                    println!(
                        "[ok]  Payment flow: 402 received, {} scheme(s), total {} {}",
                        pr.accepts.len(),
                        pr.cost_breakdown.total,
                        pr.cost_breakdown.currency
                    );
                }
                Err(e) => {
                    println!("[FAIL] Payment flow: 402 received but invalid body: {e}");
                }
            }
        }
        Ok(resp) if resp.status().is_success() => {
            println!("[ok]  Payment flow: model returned 200 (free/cached)");
        }
        Ok(resp) => {
            println!("[FAIL] Payment flow: unexpected HTTP {}", resp.status());
        }
        Err(e) => {
            println!("[FAIL] Payment flow: {e}");
        }
    }
}
