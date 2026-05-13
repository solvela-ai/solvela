"""Solvela client — smart chat flow with payment, caching, sessions, quality checks."""

from __future__ import annotations

import base64
import json
import logging
from typing import TYPE_CHECKING

from solvela.cache import ResponseCache
from solvela.config import ClientConfig
from solvela.constants import SOLANA_NETWORK, USDC_MINT
from solvela.errors import (
    AmountExceedsMaxError,
    ClientError,
    PaymentRejectedError,
    PaymentRequiredError,
    RecipientMismatchError,
)
from solvela.quality import check_degraded
from solvela.session import SessionStore
from solvela.transport import Transport
from solvela.types import (
    AtomicUsdc,
    ChatChunk,
    ChatMessage,
    ChatRequest,
    ChatResponse,
    ModelInfo,
    PaymentAccept,
    PaymentRequired,
    Role,
)

if TYPE_CHECKING:
    from collections.abc import AsyncIterator, Callable

    from solvela.signer import Signer
    from solvela.wallet import Wallet

logger = logging.getLogger(__name__)


class SolvelaClient:
    """High-level client for the Solvela gateway with smart chat flow."""

    def __init__(
        self,
        config: ClientConfig | None = None,
        wallet: Wallet | None = None,
        signer: Signer | None = None,
    ) -> None:
        self._config = config or ClientConfig()
        self._wallet = wallet
        self._signer = signer
        self._transport = Transport(
            base_url=self._config.gateway_url,
            timeout=self._config.timeout,
        )
        self._cache: ResponseCache | None = ResponseCache() if self._config.enable_cache else None
        self._session_store: SessionStore | None = (
            SessionStore(ttl=self._config.session_ttl) if self._config.enable_sessions else None
        )
        self._last_balance: float | None = None

    async def chat(self, request: ChatRequest) -> ChatResponse:
        """Non-streaming chat with full smart feature integration.

        7-step flow:
        1. Balance guard — if balance == 0 and fallback model set, swap model
        2. Session lookup — derive session, get_or_create, may override model
        3. Cache check — after model finalization, check cache
        4. Send request — with payment signing if needed
        5. Quality check — detect degraded, retry if enabled
        6. Cache store
        7. Session update
        """
        model = request.model

        # Step 1: Balance guard
        if (
            self._last_balance is not None
            and self._last_balance == 0.0
            and self._config.free_fallback_model is not None
        ):
            model = self._config.free_fallback_model

        # Step 2: Session lookup
        session_id: str | None = None
        if self._session_store is not None:
            session_id = SessionStore.derive_session_id(request.messages)
            info = self._session_store.get_or_create(session_id, model)
            if model == request.model:  # not overridden by balance guard
                model = info.model

        # Step 3: Cache check (AFTER model finalization — prevents cross-model pollution)
        cache_key: int | None = None
        if self._cache is not None:
            cache_key = ResponseCache.cache_key(model, request.messages)
            cached = self._cache.get(cache_key)
            if cached is not None:
                return cached

        # Step 4: Send request
        effective_request = ChatRequest(
            model=model,
            messages=request.messages,
            max_tokens=request.max_tokens,
            temperature=request.temperature,
            top_p=request.top_p,
            stream=False,
            tools=request.tools,
            tool_choice=request.tool_choice,
        )
        response = await self._send_with_payment(effective_request)

        # Step 5: Quality check + degraded retry
        if self._config.enable_quality_check and response.choices:
            for _ in range(self._config.max_quality_retries):
                content = response.choices[0].message.content if response.choices else ""
                reason = check_degraded(content)
                if reason is None:
                    break
                response = await self._send_with_payment(
                    effective_request,
                    extra_headers={"X-Solvela-Retry-Reason": "degraded"},
                )

        # Step 6: Cache store
        if self._cache is not None and cache_key is not None:
            self._cache.put(cache_key, response)

        # Step 7: Session update
        if self._session_store is not None and session_id is not None:
            request_hash = ResponseCache.cache_key(model, request.messages)
            self._session_store.record_request(session_id, request_hash)

        return response

    async def chat_stream(self, request: ChatRequest) -> AsyncIterator[ChatChunk]:
        """Streaming chat with balance guard + session lookup + payment handshake.

        No cache, no quality check for streaming.

        If the gateway responds with 402 on the streaming request, the client
        performs a non-streaming handshake (POST → 402 → sign → reuse signature)
        and then opens the streaming connection with the signed
        ``Payment-Signature`` header. If no signer is configured, a
        ``PaymentRequiredError`` is raised so callers cannot accidentally
        bypass payment verification on streaming endpoints.
        """
        model = request.model

        # Step 1: Balance guard
        if (
            self._last_balance is not None
            and self._last_balance == 0.0
            and self._config.free_fallback_model is not None
        ):
            model = self._config.free_fallback_model

        # Step 2: Session lookup
        session_id: str | None = None
        if self._session_store is not None:
            session_id = SessionStore.derive_session_id(request.messages)
            info = self._session_store.get_or_create(session_id, model)
            if model == request.model:
                model = info.model

        effective_request = ChatRequest(
            model=model,
            messages=request.messages,
            max_tokens=request.max_tokens,
            temperature=request.temperature,
            top_p=request.top_p,
            stream=True,
            tools=request.tools,
            tool_choice=request.tool_choice,
        )

        # Step 3: Pre-flight payment handshake.
        # The 402 challenge happens on the initial POST before SSE begins, so
        # we can probe with a non-streaming request, sign once, and reuse the
        # resulting signature for the streaming POST. This mirrors the
        # sign-and-retry flow in `_send_with_payment` but stops short of
        # consuming the response body.
        payment_signature = await self._preflight_payment_signature(effective_request)

        # If we already produced a signature, a 402 on the streaming POST is a
        # *post-signing* rejection — surface it as PaymentRejectedError (typed),
        # not the bare PaymentRequiredError that send_chat_stream raises for any
        # 402. Without this, callers cannot distinguish "needs signing" from
        # "signed and rejected" on the streaming path.
        try:
            async for chunk in self._transport.send_chat_stream(
                effective_request, payment_signature=payment_signature
            ):
                yield chunk
        except PaymentRequiredError as exc:
            if payment_signature is not None:
                raise PaymentRejectedError(
                    "second 402 after signing (streaming)",
                    payment_required=exc.payment_required,
                ) from exc
            raise

        # Step 4: Session update
        if self._session_store is not None and session_id is not None:
            request_hash = ResponseCache.cache_key(model, request.messages)
            self._session_store.record_request(session_id, request_hash)

    async def _preflight_payment_signature(self, request: ChatRequest) -> str | None:
        """Probe the gateway with a non-streaming POST to obtain a payment signature.

        Returns:
            ``None`` if the gateway does not require payment, otherwise the
            base64-encoded ``Payment-Signature`` header value to attach to the
            real streaming request.

        Raises:
            PaymentRequiredError: payment is required but no signer is configured.
            PaymentRejectedError: gateway rejected the signed streaming request.
                Surfaced by the caller (``chat_stream``) — the preflight itself
                does not re-probe to avoid doubling round-trips.

        Note:
            This implementation assumes the gateway always returns 402 before
            fulfilling a non-signed request. If the gateway ever fulfills the
            probe instead of challenging it (e.g., free first-turn quota,
            cached completion) the caller silently pays for a response that is
            then discarded — the probe body is not consumed or cached. The
            proper long-term fix is a dedicated non-billing probe endpoint;
            until then the gateway must guarantee the always-402-pre-signature
            contract. Tracked in the project issue tracker.
        """
        # Send a non-streaming probe using the same model + messages so the
        # gateway computes an identical cost breakdown. We deliberately do not
        # consume / cache the response body — we only need the 402 metadata.
        probe = ChatRequest(
            model=request.model,
            messages=request.messages,
            max_tokens=request.max_tokens,
            temperature=request.temperature,
            top_p=request.top_p,
            stream=False,
            tools=request.tools,
            tool_choice=request.tool_choice,
        )
        result = await self._transport.send_chat(probe)
        if not isinstance(result, PaymentRequired):
            return None

        if self._signer is None:
            raise PaymentRequiredError(result)

        accept = self._find_compatible_scheme(result)
        amount = self._validate_payment(accept)

        payload = await self._signer.sign_payment(
            amount_atomic=amount,
            recipient=accept.pay_to,
            resource=result.resource,
            accepted=accept,
        )
        return base64.b64encode(json.dumps(payload.to_dict()).encode()).decode()

    async def models(self) -> list[ModelInfo]:
        """Fetch available models from gateway."""
        data = await self._transport.fetch_models()
        return [ModelInfo.from_dict(m) for m in data]

    async def estimate_cost(self, model: str) -> PaymentRequired:
        """Probe a model to get cost breakdown (triggers 402)."""
        req = ChatRequest(
            model=model,
            messages=[ChatMessage(role=Role.USER, content="cost probe")],
        )
        result = await self._transport.send_chat(req)
        if isinstance(result, PaymentRequired):
            return result
        raise ClientError("Model did not return 402 — may be free tier")

    async def usdc_balance(self) -> float:
        """Query USDC-SPL balance of this client's wallet."""
        if self._wallet is None:
            raise ClientError("No wallet configured")
        return await self._query_balance(self._wallet.address())

    async def usdc_balance_of(self, address: str) -> float:
        """Query USDC balance of any Solana address."""
        return await self._query_balance(address)

    def last_known_balance(self) -> float | None:
        """Get last polled balance (from BalanceMonitor), or None."""
        return self._last_balance

    def balance_state_setter(self) -> Callable[[float | None], None]:
        """Return a callable that sets the balance state. Used by BalanceMonitor.

        Accepts ``None`` so a polling failure (raising ``ClientError``) can
        clear the cached balance back to "unknown". Without this, a stale
        ``0.0`` from a successful pre-outage poll would keep the chat balance
        guard pinned to the free-fallback model for the entire RPC outage,
        even after BalanceMonitor itself has moved on.
        """

        def set_balance(balance: float | None) -> None:
            self._last_balance = balance

        return set_balance

    # --- Private helpers ---

    async def _send_with_payment(
        self,
        request: ChatRequest,
        extra_headers: dict[str, str] | None = None,
    ) -> ChatResponse:
        """Send request, handle 402 by signing and retrying."""
        result = await self._transport.send_chat(request, extra_headers=extra_headers)

        if isinstance(result, PaymentRequired):
            if self._signer is None:
                raise PaymentRequiredError(result)

            accept = self._find_compatible_scheme(result)
            amount = self._validate_payment(accept)

            payload = await self._signer.sign_payment(
                amount_atomic=amount,
                recipient=accept.pay_to,
                resource=result.resource,
                accepted=accept,
            )
            sig = base64.b64encode(json.dumps(payload.to_dict()).encode()).decode()

            result = await self._transport.send_chat(
                request, payment_signature=sig, extra_headers=extra_headers
            )
            if isinstance(result, PaymentRequired):
                raise PaymentRejectedError(
                    "second 402 after signing",
                    payment_required=result,
                )

        return result

    def _find_compatible_scheme(self, pr: PaymentRequired) -> PaymentAccept:
        """Find first compatible payment scheme.

        Preference order is controlled by ``ClientConfig.prefer_escrow``:
        ``False`` (default) tries ``exact`` first, ``True`` tries ``escrow``
        first. Whichever is preferred falls back to the other before raising.
        """
        primary, fallback = (
            ("escrow", "exact") if self._config.prefer_escrow else ("exact", "escrow")
        )
        for accept in pr.accepts:
            if accept.scheme == primary:
                return accept
        for accept in pr.accepts:
            if accept.scheme == fallback:
                # Caller expressed a preference the gateway didn't honor — log
                # at WARNING so this is observable in production rather than a
                # silent fallback. WARNING (not ERROR) because the call still
                # proceeds successfully on the other scheme.
                logger.warning(
                    "Configured prefer_escrow=%s but gateway only offered %r; falling back",
                    self._config.prefer_escrow,
                    fallback,
                )
                return accept
        raise ClientError("No compatible payment scheme found")

    def _validate_payment(self, accept: PaymentAccept) -> AtomicUsdc:
        """Validate recipient, network, asset, and amount limits before signing.

        Network and asset are checked against expected Solana mainnet + USDC mint
        constants so a malicious or misconfigured gateway cannot trick the signer
        into authorizing a transfer on the wrong chain or with the wrong token.

        Returns the parsed atomic-unit amount so callers don't re-parse the
        wire string and can't disagree with what was validated.
        """
        if accept.network != SOLANA_NETWORK:
            # Avoid echoing the unexpected network back into logs verbatim
            # beyond what's necessary; repr keeps it bounded.
            raise ClientError(f"Unexpected payment network: {accept.network!r}")
        if accept.asset != USDC_MINT:
            # Don't echo the asset itself — keeps logs free of attacker-controlled
            # mint addresses that could otherwise be used for log-injection.
            raise ClientError("Unexpected payment asset")
        if (
            self._config.expected_recipient is not None
            and accept.pay_to != self._config.expected_recipient
        ):
            raise RecipientMismatchError(
                expected=self._config.expected_recipient,
                actual=accept.pay_to,
            )
        # Wire amount is a string; cast at this single boundary so the rest of
        # the SDK only ever sees a typed AtomicUsdc. A malformed or negative
        # value must surface as ClientError, not bare ValueError/OverflowError,
        # to honor the project's typed-error hierarchy.
        try:
            parsed = int(accept.amount)
        except (TypeError, ValueError) as exc:
            raise ClientError("Gateway returned non-integer payment amount") from exc
        if parsed < 0:
            raise ClientError("Gateway returned negative payment amount")
        amount = AtomicUsdc(parsed)
        if self._config.max_payment_amount is not None and amount > self._config.max_payment_amount:
            raise AmountExceedsMaxError(
                amount=amount,
                max_amount=self._config.max_payment_amount,
            )
        return amount

    async def _query_balance(self, address: str) -> float:
        """Query USDC token balance for an address via RPC.

        Returns ``0.0`` only when the ATA legitimately does not exist (RPC
        reports "could not find account"). Any other RPC error — rate limit,
        node down, malformed response — raises ``ClientError`` so the balance
        guard cannot be tripped silently into the free-fallback path by an
        infrastructure failure that masquerades as a zero balance.
        """
        import httpx
        from solders.pubkey import Pubkey

        from solvela.constants import USDC_MINT

        owner = Pubkey.from_string(address)
        mint = Pubkey.from_string(USDC_MINT)

        # Derive ATA
        ata_program = Pubkey.from_string("ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL")
        token_program = Pubkey.from_string("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA")
        ata, _ = Pubkey.find_program_address(
            [bytes(owner), bytes(token_program), bytes(mint)],
            ata_program,
        )

        async with httpx.AsyncClient(timeout=30) as client:
            resp = await client.post(
                self._config.rpc_url,
                json={
                    "jsonrpc": "2.0",
                    "id": 1,
                    "method": "getTokenAccountBalance",
                    "params": [str(ata)],
                },
            )
            if resp.status_code != 200:
                raise ClientError(f"USDC balance RPC HTTP {resp.status_code}")
            try:
                data = resp.json()
            except Exception as err:
                raise ClientError("USDC balance RPC: malformed JSON body") from err

            rpc_err = data.get("error") if isinstance(data, dict) else None
            if rpc_err is not None:
                # Only the "account does not exist" case is treated as a real
                # zero balance. Every other RPC error must surface so the
                # caller (or balance poller) does not silently switch to the
                # free-fallback model on a transient infrastructure issue.
                #
                # Match the canonical Solana validator phrase ("could not find
                # account") rather than the broad "not found" substring — the
                # latter also matches "Method not found" (-32601, misconfigured
                # endpoint) and "Block not found"/"Slot not found" (transient
                # node sync), neither of which means the balance is zero.
                msg = rpc_err.get("message", "") if isinstance(rpc_err, dict) else str(rpc_err)
                msg_lower = msg.lower()
                if "could not find account" in msg_lower or "account not found" in msg_lower:
                    return 0.0
                raise ClientError(f"USDC balance RPC error: {msg}")

            try:
                value = data["result"]["value"]
            except (KeyError, TypeError) as inner_err:
                raise ClientError("USDC balance RPC: unexpected response shape") from inner_err
            if value is None:
                # Some RPC providers return `result.value = null` instead of
                # an explicit error when the ATA is absent.
                return 0.0
            ui_amount = value.get("uiAmount") if isinstance(value, dict) else None
            return float(ui_amount or 0.0)

    def __repr__(self) -> str:
        return f"SolvelaClient(gateway={self._config.gateway_url}, wallet=REDACTED)"
