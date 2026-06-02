use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use ahash::AHasher;
use eventsource_stream::Eventsource;
use futures::stream::{self, Stream, StreamExt};
use reqwest::StatusCode;
use solana_sdk::pubkey::Pubkey;
use tracing::{debug, warn};

use solvela_protocol::{
    ChatChunk, ChatMessage, ChatRequest, ChatResponse, CostBreakdown, ModelInfo, PaymentRequired,
    Role, SOLANA_NETWORK, USDC_MINT,
};

use crate::cache::ResponseCache;
use crate::config::ClientConfig;
use crate::error::ClientError;
use crate::quality;
use crate::session::SessionStore;
use crate::signer;
use crate::wallet::Wallet;

/// Client for interacting with a `Solvela` gateway.
///
/// Handles the x402 payment handshake transparently: sends a probe request,
/// and if the gateway returns 402, signs a payment transaction and retries.
pub struct SolvelaClient {
    wallet: Wallet,
    config: ClientConfig,
    http: reqwest::Client,
    /// Shared atomic balance state. `u64::MAX` = not yet polled.
    balance_state: Arc<AtomicU64>,
    /// Optional response cache (created if `config.enable_cache` is true).
    cache: Option<crate::cache::ResponseCache>,
    /// Optional session store (created if `config.enable_sessions` is true).
    session_store: Option<crate::session::SessionStore>,
}

impl SolvelaClient {
    /// Create a new client with the given wallet and configuration.
    ///
    /// # Errors
    ///
    /// Returns `ClientError::Config` if the HTTP client cannot be built.
    pub fn new(wallet: Wallet, config: ClientConfig) -> Result<Self, ClientError> {
        let http = reqwest::Client::builder()
            .timeout(config.timeout)
            .build()
            .map_err(|e| ClientError::Config(format!("failed to build HTTP client: {e}")))?;

        // HIGH-2: surface a warning when the client is constructed without an
        // expected recipient. Without it, the client cannot detect a malicious
        // gateway redirecting payments to an attacker-controlled wallet.
        if config.expected_recipient.is_none() {
            warn!(
                "SolvelaClient created without `expected_recipient`; \
                 payment redirect attacks by a malicious gateway cannot be detected. \
                 Set ClientBuilder::expected_recipient to the trusted gateway wallet."
            );
        }

        let cache = if config.enable_cache {
            Some(crate::cache::ResponseCache::new())
        } else {
            None
        };

        let session_store = if config.enable_sessions {
            Some(crate::session::SessionStore::new(config.session_ttl))
        } else {
            None
        };

        Ok(Self {
            wallet,
            config,
            http,
            balance_state: Arc::new(AtomicU64::new(u64::MAX)),
            cache,
            session_store,
        })
    }

    /// Send a chat completion request with transparent 402 payment handling.
    ///
    /// Integrates smart features: response caching, free-tier fallback on zero
    /// balance, session tracking (with three-strike escalation), and degraded
    /// response detection with automatic retry.
    ///
    /// # Errors
    ///
    /// Returns `ClientError::Gateway` for non-200/402 responses,
    /// `ClientError::Signing` if payment signing fails, or
    /// `ClientError::PaymentRejected` if the gateway rejects the payment.
    pub async fn chat(&self, req: ChatRequest) -> Result<ChatResponse, ClientError> {
        let url = format!("{}/v1/chat/completions", self.config.gateway_url);
        let mut effective_req = req.clone();
        effective_req.stream = false;

        // --- Step 1: Balance guard (free fallback) ---
        let used_fallback = if let Some(ref fallback_model) = self.config.free_fallback_model {
            let balance_atomic = self.balance_state.load(Ordering::Relaxed);
            if balance_atomic == u64::MAX {
                debug!("balance not yet polled; free fallback inactive");
                false
            } else if balance_atomic == 0 {
                warn!(fallback_model = %fallback_model, "using free fallback (zero balance)");
                effective_req.model.clone_from(fallback_model);
                true
            } else {
                false
            }
        } else {
            false
        };

        // --- Step 2: Session lookup ---
        let session_id = if let Some(ref store) = self.session_store {
            let sid = SessionStore::derive_session_id(&effective_req.messages);
            let session = store.get_or_create(&sid, &effective_req.model).await;
            // TODO: act on session.escalated (e.g., upgrade model tier or add X-Solvela-Escalated header)

            // Use the session's model unless we already fell back to free tier
            if !used_fallback {
                effective_req.model.clone_from(&session.model);
            }
            Some(sid)
        } else {
            None
        };

        // --- Step 3: Cache check (after model is finalized by Steps 1-2) ---
        let cache_key = if self.cache.is_some() {
            let key = ResponseCache::cache_key(&effective_req.model, &effective_req.messages);
            if let Some(cached) = self.cache.as_ref().and_then(|c| c.get(key)) {
                debug!("cache hit for request");
                return Ok(cached);
            }
            Some(key)
        } else {
            None
        };

        // --- Step 4: Send request (existing 402 handshake) ---
        let (response, paid_atomic) = self.send_chat_request(&url, &effective_req).await?;

        // --- Step 5: Degraded detection ---
        let response = if self.config.enable_quality_check {
            self.retry_if_degraded(&url, &effective_req, response, paid_atomic)
                .await?
        } else {
            response
        };

        // --- Step 6: Cache store + session update ---
        if let (Some(key), Some(ref cache)) = (cache_key, &self.cache) {
            cache.put(key, response.clone());
        }

        if let (Some(ref sid), Some(ref store)) = (&session_id, &self.session_store) {
            let content_hash = Self::hash_request_content(&effective_req);
            store.record_request(sid, content_hash).await;
        }

        Ok(response)
    }

    /// Send a chat request, handling the 402 payment handshake if needed.
    ///
    /// Returns the response and the atomic USDC amount paid (0 if the gateway
    /// returned 200 directly, e.g. for a free/cached model). Callers use the
    /// paid amount to enforce per-request cumulative budget caps (LOW-1).
    async fn send_chat_request(
        &self,
        url: &str,
        req: &ChatRequest,
    ) -> Result<(ChatResponse, u64), ClientError> {
        let probe_resp = self.http.post(url).json(req).send().await?;
        let status = probe_resp.status();

        match status {
            StatusCode::OK => {
                debug!("gateway returned 200 directly (free/cached model)");
                let body = probe_resp.text().await?;
                let parsed: ChatResponse = serde_json::from_str(&body)
                    .map_err(|e| ClientError::ParseError(e.to_string()))?;
                Ok((parsed, 0))
            }
            StatusCode::PAYMENT_REQUIRED => {
                debug!("gateway returned 402, initiating payment");
                let body = probe_resp.text().await?;
                let payment_required: PaymentRequired = serde_json::from_str(&body)
                    .map_err(|e| ClientError::ParseError(format!("invalid 402 body: {e}")))?;
                self.pay_and_resend_with_amount(url, req, &payment_required)
                    .await
            }
            _ => {
                let body = probe_resp.text().await.unwrap_or_default();
                Err(ClientError::Gateway {
                    status: status.as_u16(),
                    message: body,
                })
            }
        }
    }

    /// Retry a request if the response is detected as degraded.
    ///
    /// `initial_paid_atomic` is the amount (atomic USDC) already spent by
    /// `send_chat_request` for the original attempt. Each retry that triggers
    /// a 402 handshake adds to a running total; if the total exceeds
    /// `config.max_payment_amount`, the retry loop short-circuits with
    /// `ClientError::BudgetExceeded` so a degraded model can't drain the
    /// caller's wallet via repeated payments (LOW-1).
    async fn retry_if_degraded(
        &self,
        url: &str,
        req: &ChatRequest,
        initial_response: ChatResponse,
        initial_paid_atomic: u64,
    ) -> Result<ChatResponse, ClientError> {
        let mut response = initial_response;
        let mut total_paid_atomic: u64 = initial_paid_atomic;
        let cap = self.config.max_payment_amount;

        for attempt in 0..self.config.max_quality_retries {
            if let Some(reason) = quality::is_degraded(&response) {
                warn!(
                    reason = ?reason,
                    attempt = attempt + 1,
                    max = self.config.max_quality_retries,
                    total_paid_atomic,
                    "degraded response detected, retrying"
                );

                let probe_resp = self
                    .http
                    .post(url)
                    .header("X-Solvela-Retry-Reason", "degraded")
                    .json(req)
                    .send()
                    .await?;
                let status = probe_resp.status();

                response = match status {
                    StatusCode::OK => {
                        let body = probe_resp.text().await?;
                        serde_json::from_str(&body)
                            .map_err(|e| ClientError::ParseError(e.to_string()))?
                    }
                    StatusCode::PAYMENT_REQUIRED => {
                        let body = probe_resp.text().await?;
                        let payment_required: PaymentRequired = serde_json::from_str(&body)
                            .map_err(|e| {
                                ClientError::ParseError(format!("invalid 402 body: {e}"))
                            })?;
                        let (paid_resp, paid_amount) = self
                            .pay_and_resend_with_amount(url, req, &payment_required)
                            .await?;
                        total_paid_atomic = total_paid_atomic.saturating_add(paid_amount);
                        if let Some(max) = cap {
                            if total_paid_atomic > max {
                                return Err(ClientError::BudgetExceeded {
                                    spent: total_paid_atomic,
                                    cap: max,
                                });
                            }
                        }
                        paid_resp
                    }
                    _ => {
                        let body = probe_resp.text().await.unwrap_or_default();
                        return Err(ClientError::Gateway {
                            status: status.as_u16(),
                            message: body,
                        });
                    }
                };
            } else {
                break;
            }
        }

        Ok(response)
    }

    /// Hash the content of all messages in a request for session tracking.
    ///
    /// Uses `AHash` for stable, deterministic per-request digests (MEDIUM-2).
    fn hash_request_content(req: &ChatRequest) -> u64 {
        let mut hasher = AHasher::default();
        for msg in &req.messages {
            msg.content.hash(&mut hasher);
        }
        hasher.finish()
    }

    /// Send a streaming chat completion request with transparent 402 payment handling.
    ///
    /// Integrates smart features: free-tier fallback on zero balance and session
    /// tracking. Cache and degraded detection are skipped for streaming because
    /// caching a stream is complex and rarely beneficial, and degraded detection
    /// requires the full response text.
    ///
    /// Returns a `Stream` of `ChatChunk` items parsed from server-sent events.
    /// The stream terminates when the server sends `[DONE]`.
    ///
    /// # Errors
    ///
    /// Returns `ClientError::Gateway` for non-200/402 responses,
    /// `ClientError::Signing` if payment signing fails, or
    /// `ClientError::StreamError` if SSE parsing fails.
    pub async fn chat_stream(
        &self,
        req: ChatRequest,
    ) -> Result<impl Stream<Item = Result<ChatChunk, ClientError>>, ClientError> {
        let url = format!("{}/v1/chat/completions", self.config.gateway_url);
        let mut effective_req = req.clone();

        // --- Step 1: Balance guard (free fallback) ---
        let used_fallback = if let Some(ref fallback_model) = self.config.free_fallback_model {
            let balance_atomic = self.balance_state.load(Ordering::Relaxed);
            if balance_atomic == u64::MAX {
                debug!("balance not yet polled; free fallback inactive");
                false
            } else if balance_atomic == 0 {
                warn!(fallback_model = %fallback_model, "using free fallback (zero balance)");
                effective_req.model.clone_from(fallback_model);
                true
            } else {
                false
            }
        } else {
            false
        };

        // --- Step 2: Session lookup ---
        if let Some(ref store) = self.session_store {
            let sid = SessionStore::derive_session_id(&effective_req.messages);
            let session = store.get_or_create(&sid, &effective_req.model).await;

            // Use the session's model unless we already fell back to free tier
            if !used_fallback {
                effective_req.model.clone_from(&session.model);
            }

            // --- Step 3: Session update before streaming ---
            // Record the request hash now since we can't do it after (stream is lazy)
            let content_hash = Self::hash_request_content(&effective_req);
            store.record_request(&sid, content_hash).await;
        }

        // Probe with stream: false to check payment status
        let mut probe_req = effective_req.clone();
        probe_req.stream = false;

        let probe_resp = self.http.post(&url).json(&probe_req).send().await?;
        let status = probe_resp.status();

        // Build the streaming request based on probe result
        effective_req.stream = true;

        let request_builder = match status {
            StatusCode::OK => {
                debug!("gateway returned 200 directly (free/cached model), opening SSE stream");
                // Discard probe body — we only needed the status
                drop(probe_resp);
                self.http.post(&url).json(&effective_req)
            }
            StatusCode::PAYMENT_REQUIRED => {
                debug!("gateway returned 402, signing payment for stream");
                let body = probe_resp.text().await?;
                let payment_required: PaymentRequired = serde_json::from_str(&body)
                    .map_err(|e| ClientError::ParseError(format!("invalid 402 body: {e}")))?;

                let payment_header = self.sign_payment_for_402(&payment_required).await?;

                self.http
                    .post(&url)
                    .header("PAYMENT-SIGNATURE", &payment_header)
                    .json(&effective_req)
            }
            _ => {
                let body = probe_resp.text().await.unwrap_or_default();
                return Err(ClientError::Gateway {
                    status: status.as_u16(),
                    message: body,
                });
            }
        };

        let stream_resp = request_builder.send().await?;
        let stream_status = stream_resp.status();
        if !stream_status.is_success() {
            let body = stream_resp.text().await.unwrap_or_default();
            return Err(ClientError::Gateway {
                status: stream_status.as_u16(),
                message: body,
            });
        }

        let es = stream_resp.bytes_stream().eventsource();

        let stream = stream::unfold(es, |mut es| async move {
            match es.next().await {
                Some(Ok(event)) => {
                    let data = event.data.trim();
                    if data == "[DONE]" {
                        return None;
                    }
                    let result: Result<ChatChunk, ClientError> = serde_json::from_str(data)
                        .map_err(|e| {
                            ClientError::StreamError(format!("failed to parse SSE chunk: {e}"))
                        });
                    Some((result, es))
                }
                Some(Err(e)) => {
                    Some((Err(ClientError::StreamError(format!("SSE error: {e}"))), es))
                }
                None => None,
            }
        });

        Ok(stream)
    }

    /// Fetch the list of available models from the gateway.
    ///
    /// # Errors
    ///
    /// Returns `ClientError::Gateway` for non-200 responses or
    /// `ClientError::ParseError` if the response body is malformed.
    pub async fn models(&self) -> Result<Vec<ModelInfo>, ClientError> {
        let url = format!("{}/v1/models", self.config.gateway_url);
        let resp = self.http.get(&url).send().await?;

        let status = resp.status();
        let body = resp.text().await?;

        if status != StatusCode::OK {
            return Err(ClientError::Gateway {
                status: status.as_u16(),
                message: body,
            });
        }
        serde_json::from_str(&body).map_err(|e| ClientError::ParseError(e.to_string()))
    }

    /// Estimate the cost of a chat request by sending a minimal probe.
    ///
    /// Sends a single-message request to trigger a 402 response and extracts
    /// the cost breakdown. The estimate reflects the gateway's per-model pricing,
    /// not a specific token count.
    ///
    /// Returns a zero-cost breakdown if the model is free (200 response).
    ///
    /// # Errors
    ///
    /// Returns `ClientError::Gateway` for unexpected status codes or
    /// `ClientError::ParseError` if the 402 body is malformed.
    pub async fn estimate_cost(&self, model: &str) -> Result<CostBreakdown, ClientError> {
        let url = format!("{}/v1/chat/completions", self.config.gateway_url);

        let probe = ChatRequest {
            model: model.to_string(),
            messages: vec![ChatMessage {
                role: Role::User,
                content: "cost estimate probe".to_string(),
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

        let resp = self.http.post(&url).json(&probe).send().await?;
        let resp_status = resp.status();
        let body = resp.text().await?;

        match resp_status {
            StatusCode::PAYMENT_REQUIRED => {
                let pr: PaymentRequired = serde_json::from_str(&body)
                    .map_err(|e| ClientError::ParseError(format!("invalid 402 body: {e}")))?;
                Ok(pr.cost_breakdown)
            }
            StatusCode::OK => Ok(CostBreakdown {
                provider_cost: "0.000000".to_string(),
                platform_fee: "0.000000".to_string(),
                total: "0.000000".to_string(),
                currency: "USDC".to_string(),
                fee_percent: 0,
            }),
            _ => Err(ClientError::Gateway {
                status: resp_status.as_u16(),
                message: body,
            }),
        }
    }

    /// Sign a payment for a 402 response and return the encoded
    /// `PAYMENT-SIGNATURE` header value.
    ///
    /// This extracts the signing flow from the 402 handshake so that
    /// external consumers (e.g., the proxy sidecar) can reuse payment
    /// signing without coupling to `ChatRequest`/`ChatResponse` types.
    ///
    /// Applies all security validations (`expected_recipient`, `max_payment_amount`).
    ///
    /// # Errors
    ///
    /// Returns `ClientError::NoCompatibleScheme` if no supported scheme is found,
    /// `ClientError::RecipientMismatch` or `ClientError::AmountExceedsMax` if
    /// security validations fail, or `ClientError::Signing` if transaction
    /// signing fails.
    pub async fn sign_payment_for_402(
        &self,
        payment_required: &PaymentRequired,
    ) -> Result<String, ClientError> {
        let (header, _amount) = self
            .sign_payment_for_402_with_amount(payment_required)
            .await?;
        Ok(header)
    }

    /// Internal variant of `sign_payment_for_402` that also returns the atomic
    /// USDC amount that was signed for, so callers can track cumulative spend
    /// (LOW-1) across multi-step request flows like `retry_if_degraded`.
    async fn sign_payment_for_402_with_amount(
        &self,
        payment_required: &PaymentRequired,
    ) -> Result<(String, u64), ClientError> {
        let accept = self
            .pick_scheme(&payment_required.accepts)
            .ok_or(ClientError::NoCompatibleScheme)?;

        // Security: validate recipient matches expected (prevents payment redirect)
        if let Some(ref expected) = self.config.expected_recipient {
            if accept.pay_to != *expected {
                return Err(ClientError::RecipientMismatch {
                    expected: expected.clone(),
                    actual: accept.pay_to.clone(),
                });
            }
        }

        let amount_atomic: u64 = accept
            .amount
            .parse()
            .map_err(|e| ClientError::ParseError(format!("invalid amount: {e}")))?;

        // Security: validate amount does not exceed maximum (prevents overcharge)
        if let Some(max) = self.config.max_payment_amount {
            if amount_atomic > max {
                return Err(ClientError::AmountExceedsMax {
                    amount: amount_atomic,
                    max,
                });
            }
        }

        let signed_tx = signer::sign_exact_payment(
            &self.wallet,
            &self.config.rpc_url,
            &self.http,
            &accept.pay_to,
            amount_atomic,
        )
        .await?;

        let payload = signer::build_payment_payload(&payment_required.resource, accept, &signed_tx);
        Ok((signer::encode_payment_header(&payload), amount_atomic))
    }

    /// Sign a payment for `payment_required`, send the paid request, and
    /// return both the response and the atomic USDC amount that was paid.
    /// Callers tracking cumulative spend (LOW-1) use the returned amount to
    /// enforce a per-request budget.
    async fn pay_and_resend_with_amount(
        &self,
        url: &str,
        req: &ChatRequest,
        payment_required: &PaymentRequired,
    ) -> Result<(ChatResponse, u64), ClientError> {
        let (payment_header, amount_atomic) = self
            .sign_payment_for_402_with_amount(payment_required)
            .await?;

        debug!("sending paid request");

        let resp = self
            .http
            .post(url)
            .header("PAYMENT-SIGNATURE", &payment_header)
            .json(req)
            .send()
            .await?;

        let status = resp.status();
        let body = resp.text().await?;

        match status {
            StatusCode::OK => {
                let parsed: ChatResponse = serde_json::from_str(&body)
                    .map_err(|e| ClientError::ParseError(e.to_string()))?;
                Ok((parsed, amount_atomic))
            }
            StatusCode::PAYMENT_REQUIRED => Err(ClientError::PaymentRejected(body)),
            _ => Err(ClientError::Gateway {
                status: status.as_u16(),
                message: body,
            }),
        }
    }

    /// Query the USDC-SPL balance of this client's wallet.
    ///
    /// # Errors
    ///
    /// Returns `ClientError::BalanceError` if the RPC call fails.
    pub async fn usdc_balance(&self) -> Result<f64, ClientError> {
        self.usdc_balance_of(&self.wallet.address()).await
    }

    /// Query the USDC-SPL balance of an arbitrary Solana address.
    ///
    /// Returns `0.0` if the associated token account does not exist.
    ///
    /// # Errors
    ///
    /// Returns `ClientError::BalanceError` if the address is invalid or the
    /// RPC call fails for a reason other than "account not found".
    pub async fn usdc_balance_of(&self, address: &str) -> Result<f64, ClientError> {
        let owner: Pubkey = address
            .parse()
            .map_err(|e| ClientError::BalanceError(format!("invalid address: {e}")))?;

        let mint: Pubkey = USDC_MINT
            .parse()
            .map_err(|e| ClientError::BalanceError(format!("invalid USDC mint: {e}")))?;

        let ata = signer::associated_token_address(&owner, &mint);

        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "getTokenAccountBalance",
            "params": [ata.to_string()]
        });

        let resp = self
            .http
            .post(&self.config.rpc_url)
            .json(&body)
            .send()
            .await
            .map_err(|e| ClientError::BalanceError(e.to_string()))?;

        let status = resp.status();
        if !status.is_success() {
            return Err(ClientError::BalanceError(format!(
                "RPC returned HTTP {status}"
            )));
        }

        let json: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| ClientError::BalanceError(format!("failed to parse RPC response: {e}")))?;

        // Discriminate the benign "account does not exist" case from real RPC
        // errors (rate limits, auth failures, malformed requests). Silently
        // returning Ok(0) on every error makes a malfunctioning RPC look like
        // an empty wallet, which is a confusing operator experience and hides
        // outages from retry loops. See issue #323.
        if let Some(error) = json.get("error") {
            if crate::rpc_error::is_account_not_found(&json) {
                return Ok(0.0);
            }
            return Err(ClientError::BalanceError(format!(
                "RPC returned error: {error}"
            )));
        }

        // At this point the response carried no `error` object, so the account
        // exists and the body must contain `result.value`. A missing
        // `result`/`value` means a structurally unexpected response (proxy
        // error page, truncated body, wrong RPC method) — surface it as an
        // error rather than silently reporting a 0 balance, which would
        // wrongly trip the free-tier fallback and suppress payments.
        let value = json
            .get("result")
            .and_then(|r| r.get("value"))
            .ok_or_else(|| {
                ClientError::BalanceError(
                    "RPC response missing result.value (unexpected shape)".to_string(),
                )
            })?;

        // Prefer the deprecated `uiAmount` float when present, but it may be
        // `null`; fall back to the authoritative `amount` (atomic string) +
        // `decimals`, which `getTokenAccountBalance` always returns for an
        // existing account. Only a response missing BOTH is treated as an
        // error — a real zero balance still parses as 0.0 here.
        let ui_amount = if let Some(n) = value.get("uiAmount").and_then(serde_json::Value::as_f64) {
            n
        } else if let (Some(amount), Some(decimals)) = (
            value.get("amount").and_then(serde_json::Value::as_str),
            value.get("decimals").and_then(serde_json::Value::as_u64),
        ) {
            let raw: u64 = amount.parse().map_err(|e| {
                ClientError::BalanceError(format!("invalid token amount '{amount}': {e}"))
            })?;
            #[allow(clippy::cast_precision_loss)] // USDC amounts fit within the f64 mantissa
            let ui = raw as f64 / 10f64.powi(i32::try_from(decimals).unwrap_or(6));
            ui
        } else {
            return Err(ClientError::BalanceError(
                "RPC response missing balance fields (uiAmount/amount)".to_string(),
            ));
        };

        Ok(ui_amount)
    }

    /// Return the last known USDC balance, or `None` if it has never been polled.
    ///
    /// Reads the shared `AtomicU64` without blocking. The value is in atomic
    /// USDC units (1 USDC = 1,000,000 atomic). Returns `None` if the sentinel
    /// value `u64::MAX` is present (meaning the balance has not been polled yet).
    #[must_use]
    #[allow(clippy::cast_precision_loss)] // USDC amounts fit well within f64 mantissa
    pub fn last_known_balance(&self) -> Option<f64> {
        let raw = self.balance_state.load(Ordering::Relaxed);
        if raw == u64::MAX {
            None
        } else {
            Some(raw as f64 / 1_000_000.0)
        }
    }

    /// Return a clone of the shared balance state `Arc<AtomicU64>`.
    ///
    /// Pass this to `BalanceMonitor::new()` so the monitor can update the
    /// balance in the background while the client reads it lock-free.
    #[must_use]
    pub fn balance_state(&self) -> Arc<AtomicU64> {
        Arc::clone(&self.balance_state)
    }

    /// Pick the best compatible payment scheme from the 402 accepts list.
    ///
    /// Currently only "exact" (direct transfer) is supported. Escrow signing
    /// is not yet implemented — escrow schemes are filtered out even when
    /// `prefer_escrow` is true. Once `sign_escrow_payment` is added, this
    /// method should respect `config.prefer_escrow` ordering.
    ///
    /// Security: validates `network` and `asset` in addition to `scheme` so
    /// that a malicious gateway cannot trick the client into signing a
    /// payment on an unexpected chain or in an unexpected token (HIGH-1).
    fn pick_scheme<'a>(
        &self,
        accepts: &'a [solvela_protocol::PaymentAccept],
    ) -> Option<&'a solvela_protocol::PaymentAccept> {
        // TODO: respect self.config.prefer_escrow once escrow signing is implemented
        let _ = self.config.prefer_escrow;
        // Only exact scheme is implemented; escrow signing is not yet available.
        // Reject any accept whose network or asset doesn't match Solana mainnet
        // USDC-SPL — the only combination this client can sign for.
        accepts
            .iter()
            .find(|a| a.scheme == "exact" && a.network == SOLANA_NETWORK && a.asset == USDC_MINT)
    }
}

impl std::fmt::Debug for SolvelaClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SolvelaClient")
            .field("wallet", &self.wallet)
            .field("gateway_url", &self.config.gateway_url)
            .field("cache_enabled", &self.cache.is_some())
            .field("sessions_enabled", &self.session_store.is_some())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt;
    use solvela_protocol::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn test_wallet() -> Wallet {
        let (wallet, _) = Wallet::create();
        wallet
    }

    fn sample_chat_request() -> ChatRequest {
        ChatRequest {
            model: "openai/gpt-4o".to_string(),
            messages: vec![ChatMessage {
                role: Role::User,
                content: "Hello".to_string(),
                name: None,
                tool_calls: None,
                tool_call_id: None,
            }],
            max_tokens: Some(100),
            temperature: None,
            top_p: None,
            stream: false,
            tools: None,
            tool_choice: None,
        }
    }

    fn sample_payment_required() -> PaymentRequired {
        PaymentRequired {
            x402_version: X402_VERSION,
            resource: Resource {
                url: "/v1/chat/completions".to_string(),
                method: "POST".to_string(),
            },
            accepts: vec![PaymentAccept {
                scheme: "exact".to_string(),
                network: SOLANA_NETWORK.to_string(),
                amount: "2625".to_string(),
                asset: USDC_MINT.to_string(),
                pay_to: solana_sdk::pubkey::Pubkey::new_unique().to_string(),
                max_timeout_seconds: MAX_TIMEOUT_SECONDS,
                escrow_program_id: None,
            }],
            cost_breakdown: CostBreakdown {
                provider_cost: "0.002500".to_string(),
                platform_fee: "0.000125".to_string(),
                total: "0.002625".to_string(),
                currency: "USDC".to_string(),
                fee_percent: PLATFORM_FEE_PERCENT,
            },
            error: "Payment required".to_string(),
        }
    }

    fn sample_chat_response() -> ChatResponse {
        ChatResponse {
            id: "chatcmpl-test123".to_string(),
            object: "chat.completion".to_string(),
            created: 1_234_567_890,
            model: "gpt-4o".to_string(),
            choices: vec![ChatChoice {
                index: 0,
                message: ChatMessage {
                    role: Role::Assistant,
                    content: "Hello! How can I help?".to_string(),
                    name: None,
                    tool_calls: None,
                    tool_call_id: None,
                },
                finish_reason: Some("stop".to_string()),
            }],
            usage: Some(Usage {
                prompt_tokens: 10,
                completion_tokens: 8,
                total_tokens: 18,
            }),
        }
    }

    #[tokio::test]
    async fn test_chat_returns_200_directly_for_free_model() {
        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(sample_chat_response()))
            .expect(1)
            .mount(&mock_server)
            .await;

        let client = SolvelaClient::new(
            test_wallet(),
            ClientConfig {
                gateway_url: mock_server.uri(),
                ..ClientConfig::default()
            },
        )
        .unwrap();

        let resp = client.chat(sample_chat_request()).await.unwrap();
        assert_eq!(resp.choices[0].message.content, "Hello! How can I help?");
    }

    #[tokio::test]
    async fn test_chat_handles_402_then_fails_signing_without_rpc() {
        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(402).set_body_json(sample_payment_required()))
            .mount(&mock_server)
            .await;

        let client = SolvelaClient::new(
            test_wallet(),
            ClientConfig {
                gateway_url: mock_server.uri(),
                rpc_url: "http://127.0.0.1:1".to_string(),
                timeout: std::time::Duration::from_secs(5),
                ..ClientConfig::default()
            },
        )
        .unwrap();

        let result = client.chat(sample_chat_request()).await;
        assert!(result.is_err());
        // Signing fails because the RPC endpoint is unreachable — the reqwest
        // error propagates through SignerError::RpcError → ClientError::Signing.
        assert!(matches!(result.unwrap_err(), ClientError::Signing(_)));
    }

    #[tokio::test]
    async fn test_chat_returns_gateway_error_for_500() {
        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(500).set_body_string("internal server error"))
            .expect(1)
            .mount(&mock_server)
            .await;

        let client = SolvelaClient::new(
            test_wallet(),
            ClientConfig {
                gateway_url: mock_server.uri(),
                ..ClientConfig::default()
            },
        )
        .unwrap();

        let result = client.chat(sample_chat_request()).await;
        match result.unwrap_err() {
            ClientError::Gateway { status, .. } => assert_eq!(status, 500),
            other => panic!("expected Gateway error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_models_returns_model_list() {
        let mock_server = MockServer::start().await;

        let models = vec![ModelInfo {
            id: "openai/gpt-4o".to_string(),
            provider: "openai".to_string(),
            display_name: "GPT-4o".to_string(),
            input_cost_per_million: 2.5,
            output_cost_per_million: 10.0,
            context_window: 128_000,
            supports_streaming: true,
            supports_tools: true,
            supports_vision: true,
            reasoning: false,
        }];

        Mock::given(method("GET"))
            .and(path("/v1/models"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&models))
            .expect(1)
            .mount(&mock_server)
            .await;

        let client = SolvelaClient::new(
            test_wallet(),
            ClientConfig {
                gateway_url: mock_server.uri(),
                ..ClientConfig::default()
            },
        )
        .unwrap();

        let result = client.models().await.unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].id, "openai/gpt-4o");
    }

    #[tokio::test]
    async fn test_sign_payment_for_402_rejects_unknown_scheme() {
        let mock_server = MockServer::start().await;
        let client = SolvelaClient::new(
            test_wallet(),
            ClientConfig {
                gateway_url: mock_server.uri(),
                ..ClientConfig::default()
            },
        )
        .unwrap();

        let mut pr = sample_payment_required();
        pr.accepts[0].scheme = "unknown".to_string();

        let result = client.sign_payment_for_402(&pr).await;
        assert!(matches!(
            result.unwrap_err(),
            ClientError::NoCompatibleScheme
        ));
    }

    #[tokio::test]
    async fn test_sign_payment_for_402_rejects_wrong_network() {
        // HIGH-1: pick_scheme must reject accepts whose network is not Solana.
        let mock_server = MockServer::start().await;
        let client = SolvelaClient::new(
            test_wallet(),
            ClientConfig {
                gateway_url: mock_server.uri(),
                ..ClientConfig::default()
            },
        )
        .unwrap();

        let mut pr = sample_payment_required();
        pr.accepts[0].network = "ethereum:1".to_string();

        let result = client.sign_payment_for_402(&pr).await;
        assert!(matches!(
            result.unwrap_err(),
            ClientError::NoCompatibleScheme
        ));
    }

    #[tokio::test]
    async fn test_sign_payment_for_402_rejects_wrong_asset() {
        // HIGH-1: pick_scheme must reject accepts whose asset is not USDC mint.
        let mock_server = MockServer::start().await;
        let client = SolvelaClient::new(
            test_wallet(),
            ClientConfig {
                gateway_url: mock_server.uri(),
                ..ClientConfig::default()
            },
        )
        .unwrap();

        let mut pr = sample_payment_required();
        // Replace asset with a different (well-formed but wrong) mint.
        pr.accepts[0].asset = solana_sdk::pubkey::Pubkey::new_unique().to_string();

        let result = client.sign_payment_for_402(&pr).await;
        assert!(matches!(
            result.unwrap_err(),
            ClientError::NoCompatibleScheme
        ));
    }

    #[tokio::test]
    async fn test_usdc_balance_of_parses_rpc_response() {
        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "result": {
                    "context": { "slot": 123_456 },
                    "value": {
                        "amount": "1500000",
                        "decimals": 6,
                        "uiAmount": 1.5,
                        "uiAmountString": "1.5"
                    }
                }
            })))
            .expect(1)
            .mount(&mock_server)
            .await;

        let client = SolvelaClient::new(
            test_wallet(),
            ClientConfig {
                gateway_url: "http://unused".to_string(),
                rpc_url: mock_server.uri(),
                ..ClientConfig::default()
            },
        )
        .unwrap();

        let balance = client
            .usdc_balance_of(&client.wallet.address())
            .await
            .unwrap();
        assert!((balance - 1.5).abs() < f64::EPSILON);
    }

    #[tokio::test]
    async fn test_usdc_balance_falls_back_to_amount_when_ui_amount_null() {
        // `uiAmount` is a deprecated field that can be `null`; the balance must
        // be computed from the authoritative `amount` + `decimals`, NOT treated
        // as 0.0. 2_500_000 atomic / 10^6 = 2.5 USDC.
        let mock_server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "result": {
                    "context": { "slot": 1 },
                    "value": {
                        "amount": "2500000",
                        "decimals": 6,
                        "uiAmount": serde_json::Value::Null,
                        "uiAmountString": "2.5"
                    }
                }
            })))
            .expect(1)
            .mount(&mock_server)
            .await;

        let client = SolvelaClient::new(
            test_wallet(),
            ClientConfig {
                gateway_url: "http://unused".to_string(),
                rpc_url: mock_server.uri(),
                ..ClientConfig::default()
            },
        )
        .unwrap();

        let balance = client
            .usdc_balance_of(&client.wallet.address())
            .await
            .unwrap();
        assert!((balance - 2.5).abs() < f64::EPSILON);
    }

    #[tokio::test]
    async fn test_usdc_balance_errors_on_malformed_response_not_silent_zero() {
        // No `error` object and no `result.value` — a structurally unexpected
        // body (e.g. a proxy error page). This MUST be an error, not a silent
        // 0.0 that would trip the free-tier fallback and suppress payments.
        let mock_server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "result": { "context": { "slot": 1 } }
            })))
            .expect(1)
            .mount(&mock_server)
            .await;

        let client = SolvelaClient::new(
            test_wallet(),
            ClientConfig {
                gateway_url: "http://unused".to_string(),
                rpc_url: mock_server.uri(),
                ..ClientConfig::default()
            },
        )
        .unwrap();

        let result = client.usdc_balance_of(&client.wallet.address()).await;
        assert!(
            matches!(result, Err(ClientError::BalanceError(_))),
            "malformed RPC response must error, got {result:?}"
        );
    }

    #[tokio::test]
    async fn test_usdc_balance_returns_zero_for_missing_account() {
        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "error": {
                    "code": -32602,
                    "message": "Invalid param: could not find account"
                }
            })))
            .expect(1)
            .mount(&mock_server)
            .await;

        let client = SolvelaClient::new(
            test_wallet(),
            ClientConfig {
                gateway_url: "http://unused".to_string(),
                rpc_url: mock_server.uri(),
                ..ClientConfig::default()
            },
        )
        .unwrap();

        let balance = client.usdc_balance().await.unwrap();
        assert!((balance - 0.0).abs() < f64::EPSILON);
    }

    #[tokio::test]
    async fn test_usdc_balance_propagates_real_rpc_errors() {
        // A non-account-not-found RPC error (e.g., rate limit) must propagate as
        // Err(ClientError::BalanceError), not be silenced to Ok(0). See #323.
        let mock_server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "error": { "code": -32005, "message": "Server is busy" }
            })))
            .expect(1)
            .mount(&mock_server)
            .await;

        let client = SolvelaClient::new(
            test_wallet(),
            ClientConfig {
                gateway_url: "http://unused".to_string(),
                rpc_url: mock_server.uri(),
                ..ClientConfig::default()
            },
        )
        .unwrap();

        let result = client.usdc_balance().await;
        assert!(result.is_err(), "rate-limit error must propagate as Err");
        match result.unwrap_err() {
            ClientError::BalanceError(msg) => {
                assert!(
                    msg.contains("Server is busy"),
                    "error should surface the upstream message: {msg}"
                );
            }
            other => panic!("expected ClientError::BalanceError, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_usdc_balance_of_invalid_address() {
        let client = SolvelaClient::new(test_wallet(), ClientConfig::default()).unwrap();

        let result = client.usdc_balance_of("not-a-valid-pubkey").await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), ClientError::BalanceError(_)));
    }

    #[tokio::test]
    async fn test_estimate_cost_uses_402_probe() {
        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(402).set_body_json(sample_payment_required()))
            .expect(1)
            .mount(&mock_server)
            .await;

        let client = SolvelaClient::new(
            test_wallet(),
            ClientConfig {
                gateway_url: mock_server.uri(),
                ..ClientConfig::default()
            },
        )
        .unwrap();

        let cost = client.estimate_cost("openai/gpt-4o").await.unwrap();
        assert_eq!(cost.total, "0.002625");
        assert_eq!(cost.currency, "USDC");
    }

    #[tokio::test]
    async fn test_chat_stream_returns_chunks_for_free_model() {
        let mock_server = MockServer::start().await;

        let sse_body = "\
data: {\"id\":\"c1\",\"object\":\"chat.completion.chunk\",\"created\":1234,\"model\":\"test\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"Hello\"},\"finish_reason\":null}]}\n\n\
data: {\"id\":\"c1\",\"object\":\"chat.completion.chunk\",\"created\":1234,\"model\":\"test\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\" world\"},\"finish_reason\":null}]}\n\n\
data: [DONE]\n\n";

        // Both probe and streaming requests return 200 with SSE content-type.
        // The probe only checks the status code (200 = free model), so the
        // body/content-type is irrelevant. The streaming request needs SSE.
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(sse_body, "text/event-stream"))
            .mount(&mock_server)
            .await;

        let client = SolvelaClient::new(
            test_wallet(),
            ClientConfig {
                gateway_url: mock_server.uri(),
                ..ClientConfig::default()
            },
        )
        .unwrap();

        let stream = client.chat_stream(sample_chat_request()).await.unwrap();
        let chunks: Vec<ChatChunk> = stream
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();

        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].choices[0].delta.content.as_deref(), Some("Hello"));
        assert_eq!(
            chunks[1].choices[0].delta.content.as_deref(),
            Some(" world")
        );
    }

    #[tokio::test]
    async fn test_chat_stream_handles_402_then_fails_signing() {
        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(402).set_body_json(sample_payment_required()))
            .mount(&mock_server)
            .await;

        let client = SolvelaClient::new(
            test_wallet(),
            ClientConfig {
                gateway_url: mock_server.uri(),
                rpc_url: "http://127.0.0.1:1".to_string(),
                timeout: std::time::Duration::from_secs(5),
                ..ClientConfig::default()
            },
        )
        .unwrap();

        let result = client.chat_stream(sample_chat_request()).await;
        assert!(result.is_err());
        match result {
            Err(ClientError::Signing(_)) => {} // expected
            Err(other) => panic!("expected Signing error, got {other:?}"),
            Ok(_) => panic!("expected error, got Ok"),
        }
    }

    #[test]
    fn test_last_known_balance_returns_none_before_poll() {
        let client = SolvelaClient::new(test_wallet(), ClientConfig::default()).unwrap();
        assert!(client.last_known_balance().is_none());
    }

    #[test]
    fn test_last_known_balance_reads_atomic() {
        let client = SolvelaClient::new(test_wallet(), ClientConfig::default()).unwrap();
        // Simulate a balance monitor writing 1.5 USDC (1_500_000 atomic)
        client
            .balance_state
            .store(1_500_000, std::sync::atomic::Ordering::Relaxed);
        let balance = client.last_known_balance().unwrap();
        assert!((balance - 1.5).abs() < f64::EPSILON);
    }

    #[test]
    fn test_balance_state_returns_shared_arc() {
        let client = SolvelaClient::new(test_wallet(), ClientConfig::default()).unwrap();
        let state = client.balance_state();
        state.store(2_000_000, std::sync::atomic::Ordering::Relaxed);
        let balance = client.last_known_balance().unwrap();
        assert!((balance - 2.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_cache_created_when_enabled() {
        let config = ClientConfig {
            enable_cache: true,
            ..ClientConfig::default()
        };
        let client = SolvelaClient::new(test_wallet(), config).unwrap();
        assert!(client.cache.is_some());
    }

    #[test]
    fn test_cache_not_created_when_disabled() {
        let client = SolvelaClient::new(test_wallet(), ClientConfig::default()).unwrap();
        assert!(client.cache.is_none());
    }

    #[test]
    fn test_session_store_created_when_enabled() {
        let config = ClientConfig {
            enable_sessions: true,
            ..ClientConfig::default()
        };
        let client = SolvelaClient::new(test_wallet(), config).unwrap();
        assert!(client.session_store.is_some());
    }

    #[tokio::test]
    async fn test_chat_cache_hit_returns_cached_response() {
        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(sample_chat_response()))
            .expect(1)
            .mount(&mock_server)
            .await;

        let client = SolvelaClient::new(
            test_wallet(),
            ClientConfig {
                gateway_url: mock_server.uri(),
                enable_cache: true,
                ..ClientConfig::default()
            },
        )
        .unwrap();

        let resp1 = client.chat(sample_chat_request()).await.unwrap();
        assert_eq!(resp1.choices[0].message.content, "Hello! How can I help?");

        // Second call should come from cache (mock expects exactly 1 call)
        let resp2 = client.chat(sample_chat_request()).await.unwrap();
        assert_eq!(resp2.choices[0].message.content, "Hello! How can I help?");
    }

    #[tokio::test]
    async fn test_chat_free_fallback_on_zero_balance() {
        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(sample_chat_response()))
            .expect(1)
            .mount(&mock_server)
            .await;

        let client = SolvelaClient::new(
            test_wallet(),
            ClientConfig {
                gateway_url: mock_server.uri(),
                free_fallback_model: Some("openai/gpt-oss-120b".to_string()),
                ..ClientConfig::default()
            },
        )
        .unwrap();

        client
            .balance_state
            .store(0, std::sync::atomic::Ordering::Relaxed);

        let resp = client.chat(sample_chat_request()).await.unwrap();
        assert_eq!(resp.choices[0].message.content, "Hello! How can I help?");
    }

    #[tokio::test]
    async fn test_chat_no_fallback_when_balance_positive() {
        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(sample_chat_response()))
            .expect(1)
            .mount(&mock_server)
            .await;

        let client = SolvelaClient::new(
            test_wallet(),
            ClientConfig {
                gateway_url: mock_server.uri(),
                free_fallback_model: Some("openai/gpt-oss-120b".to_string()),
                ..ClientConfig::default()
            },
        )
        .unwrap();

        client
            .balance_state
            .store(5_000_000, std::sync::atomic::Ordering::Relaxed);

        let resp = client.chat(sample_chat_request()).await.unwrap();
        assert_eq!(resp.choices[0].message.content, "Hello! How can I help?");
    }

    #[tokio::test]
    async fn test_chat_no_fallback_when_not_configured() {
        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(sample_chat_response()))
            .expect(1)
            .mount(&mock_server)
            .await;

        let client = SolvelaClient::new(
            test_wallet(),
            ClientConfig {
                gateway_url: mock_server.uri(),
                ..ClientConfig::default()
            },
        )
        .unwrap();

        client
            .balance_state
            .store(0, std::sync::atomic::Ordering::Relaxed);

        let resp = client.chat(sample_chat_request()).await.unwrap();
        assert_eq!(resp.choices[0].message.content, "Hello! How can I help?");
    }

    #[tokio::test]
    async fn test_chat_degraded_retry() {
        let mock_server = MockServer::start().await;

        let degraded_response = ChatResponse {
            id: "degraded".to_string(),
            object: "chat.completion".to_string(),
            created: 1_000_000,
            model: "test".to_string(),
            choices: vec![ChatChoice {
                index: 0,
                message: ChatMessage {
                    role: Role::Assistant,
                    content: "As an AI language model, I can help you.".to_string(),
                    name: None,
                    tool_calls: None,
                    tool_call_id: None,
                },
                finish_reason: Some("stop".to_string()),
            }],
            usage: None,
        };

        let good_response = sample_chat_response();

        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&degraded_response))
            .up_to_n_times(1)
            .mount(&mock_server)
            .await;

        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&good_response))
            .mount(&mock_server)
            .await;

        let client = SolvelaClient::new(
            test_wallet(),
            ClientConfig {
                gateway_url: mock_server.uri(),
                enable_quality_check: true,
                max_quality_retries: 1,
                ..ClientConfig::default()
            },
        )
        .unwrap();

        let resp = client.chat(sample_chat_request()).await.unwrap();
        assert_eq!(resp.choices[0].message.content, "Hello! How can I help?");
    }

    #[tokio::test]
    async fn test_chat_stream_with_free_fallback() {
        let mock_server = MockServer::start().await;

        let sse_body = "\
data: {\"id\":\"c1\",\"object\":\"chat.completion.chunk\",\"created\":1234,\"model\":\"free-model\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"Free\"},\"finish_reason\":null}]}\n\n\
data: [DONE]\n\n";

        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(sse_body, "text/event-stream"))
            .mount(&mock_server)
            .await;

        let client = SolvelaClient::new(
            test_wallet(),
            ClientConfig {
                gateway_url: mock_server.uri(),
                free_fallback_model: Some("openai/gpt-oss-120b".to_string()),
                ..ClientConfig::default()
            },
        )
        .unwrap();

        client
            .balance_state
            .store(0, std::sync::atomic::Ordering::Relaxed);

        let stream = client.chat_stream(sample_chat_request()).await.unwrap();
        let chunks: Vec<ChatChunk> = stream
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();

        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].choices[0].delta.content.as_deref(), Some("Free"));
    }
}
