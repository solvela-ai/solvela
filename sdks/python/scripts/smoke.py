"""Manual smoke test — exercises the real wire contract before release.

Run before tagging a release to verify the SDK still agrees with a live
Solvela gateway. Catches wire-format drift that unit/integration tests
cannot (header names, JSON field renames, accepts-array ordering, new
required fields).

Usage:
    SOLVELA_GATEWAY_URL=https://staging.solvela.ai \
    .venv/bin/python scripts/smoke.py

Defaults to http://localhost:8402 if SOLVELA_GATEWAY_URL is unset.

Exit codes:
    0 — all assertions passed
    1 — an assertion failed or the gateway is unreachable
"""

from __future__ import annotations

import asyncio
import os
import re
import sys

from solvela.client import SolvelaClient
from solvela.config import ClientConfig
from solvela.constants import SOLANA_NETWORK, USDC_MINT, X402_VERSION
from solvela.errors import ClientError, PaymentRequiredError
from solvela.types import ChatMessage, ChatRequest, Role

# Solana base58 pubkeys: 32-44 chars over the Bitcoin base58 alphabet
# (no 0/O/I/l). An empty or malformed pay_to would route signed funds to
# address-zero — the most expensive class of silent drift.
SOLANA_PUBKEY_RE = re.compile(r"^[1-9A-HJ-NP-Za-km-z]{32,44}$")


async def main() -> int:
    gateway_url = os.environ.get("SOLVELA_GATEWAY_URL", "http://localhost:8402")
    print(f"Smoke test against: {gateway_url}\n")

    try:
        client = SolvelaClient(config=ClientConfig(gateway_url=gateway_url))
    except ClientError as exc:
        print(f"FAIL: client construction rejected gateway URL: {exc}")
        return 1

    # 1. models() reaches the gateway and parses the response.
    try:
        models = await client.models()
    except Exception as exc:
        print(f"FAIL: models() raised {type(exc).__name__}: {exc}")
        return 1
    print(f"  models()           -> {len(models)} model(s) returned")
    if not models:
        print("FAIL: expected at least one model from the gateway")
        return 1
    sample_model = models[0]
    print(
        f"  sample model       -> id={sample_model.id!r} "
        f"ctx={sample_model.context_window} "
        f"in={sample_model.input_usdc_per_million}/M "
        f"out={sample_model.output_usdc_per_million}/M"
    )

    # Guard against silent ModelInfo wire drift: every prior shape change
    # surfaced as all-zero pricing / all-false capabilities. Assert at least
    # one model in the registry exposes streaming and paid input pricing.
    if not any(m.supports_streaming for m in models):
        print(
            "FAIL: no model reports supports_streaming=True — capabilities parsing may be drifted"
        )
        return 1
    if not any(m.input_usdc_per_million > 0 for m in models):
        print("FAIL: no model reports input_usdc_per_million > 0 — pricing parsing may be drifted")
        return 1
    if not sample_model.display_name or not sample_model.provider:
        print("FAIL: sample model missing display_name or provider")
        return 1

    # 2. Unsigned chat returns 402 with a parseable PaymentRequired body.
    req = ChatRequest(
        model=sample_model.id,
        messages=[ChatMessage(role=Role.USER, content="ping")],
    )
    try:
        await client.chat(req)
    except PaymentRequiredError as exc:
        pr = exc.payment_required
        total = pr.cost_breakdown.total
        currency = pr.cost_breakdown.currency
        schemes = [a.scheme for a in pr.accepts]
        print(f"  chat() unsigned    -> 402 OK (total={total} {currency}, schemes={schemes})")
        if not pr.accepts:
            print("FAIL: 402 response had empty accepts array")
            return 1

        # Critical drift checks — a silent regression in any of these would
        # route real funds wrong. All six are derivable from the unsigned 402
        # we just received, so no extra gateway round-trip is required.
        accept = pr.accepts[0]
        if not accept.pay_to or not SOLANA_PUBKEY_RE.match(accept.pay_to):
            print(f"FAIL: accepts[0].pay_to invalid: {accept.pay_to!r:.64}")
            return 1
        if not re.fullmatch(r"\d+", accept.amount) or int(accept.amount) <= 0:
            print(
                "FAIL: accepts[0].amount must be a positive decimal-integer string: "
                f"{accept.amount!r:.32}"
            )
            return 1
        if accept.network != SOLANA_NETWORK:
            print(f"FAIL: accepts[0].network={accept.network!r} (expected {SOLANA_NETWORK!r})")
            return 1
        if accept.asset != USDC_MINT:
            print(f"FAIL: accepts[0].asset={accept.asset!r} (expected {USDC_MINT!r})")
            return 1
        if pr.cost_breakdown.currency != "USDC":
            print(f"FAIL: cost_breakdown.currency={pr.cost_breakdown.currency!r} (expected 'USDC')")
            return 1
        if pr.x402_version != X402_VERSION:
            print(f"FAIL: x402_version={pr.x402_version} (SDK expects {X402_VERSION})")
            return 1
        print(
            "  critical checks    -> pay_to OK amount OK network OK asset OK currency OK "
            f"x402_version={X402_VERSION} OK"
        )
    except Exception as exc:
        print(f"FAIL: chat() raised {type(exc).__name__} (expected PaymentRequiredError): {exc}")
        return 1
    else:
        print("FAIL: chat() returned a response with no signer configured (expected 402)")
        return 1

    print("\nSmoke test PASSED")
    return 0


if __name__ == "__main__":
    sys.exit(asyncio.run(main()))
