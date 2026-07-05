"""Cross-SDK 402-parse smoke (channel plan PR-B, invariant 12 / HALT 12).

Parses the LIVE gateway 402 challenge fixture shared with the TS and Go SDK
suites (``crates/gateway/tests/fixtures/chat_402_challenge.json``, pinned
byte-shape-identical to the real route by the gateway integration test
``x402_challenge_smoke_tests::live_402_body_matches_cross_sdk_fixture``).

``PaymentAccept.from_dict`` rejects the ENTIRE 402 on any unknown scheme, so a
gateway that ever advertised a new scheme (e.g. ``channel``) in ``accepts[]``
would break every deployed paid call at parse time — the memorialized
cross-repo wire-drift failure mode. This smoke is the standing tripwire.
"""

from __future__ import annotations

import copy
import json
from pathlib import Path

import pytest

from solvela.errors import ClientError
from solvela.types import PaymentRequired

_FIXTURE = (
    Path(__file__).resolve().parents[4]
    / "crates"
    / "gateway"
    / "tests"
    / "fixtures"
    / "chat_402_challenge.json"
)


def _load_fixture() -> dict:
    return json.loads(_FIXTURE.read_text())


def test_live_402_fixture_parses() -> None:
    pr = PaymentRequired.from_dict(_load_fixture())
    assert pr.accepts, "fixture must advertise at least one accepts entry"
    for accept in pr.accepts:
        assert accept.scheme in ("exact", "escrow")
    assert pr.resource.url == "/v1/chat/completions"
    assert pr.cost_breakdown.currency == "USDC"


def test_live_402_fixture_with_channel_scheme_rejected() -> None:
    data = _load_fixture()
    channel_entry = copy.deepcopy(data["accepts"][0])
    channel_entry["scheme"] = "channel"
    data["accepts"].append(channel_entry)
    # The rejection takes down the WHOLE challenge parse — exact users
    # included — which is exactly the invariant-12 blast radius and why the
    # gateway must never advertise 'channel' in accepts[] (HALT 12).
    with pytest.raises(ClientError):
        PaymentRequired.from_dict(data)
