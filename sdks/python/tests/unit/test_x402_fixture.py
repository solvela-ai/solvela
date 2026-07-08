"""Cross-SDK 402-parse smoke (channel plan PR-B / Slice B).

Parses the LIVE gateway 402 challenge fixture shared with the TS and Go SDK
suites (``crates/gateway/tests/fixtures/chat_402_challenge.json``, pinned
byte-shape-identical to the real route by the gateway integration test
``x402_challenge_smoke_tests::live_402_body_matches_cross_sdk_fixture``).

Since Slice B (channel plan §7), ``PaymentRequired.from_dict`` SKIPS unknown-
scheme entries (representing them in ``unrecognized_schemes``) instead of
rejecting the whole 402 — a gateway advertising a new scheme (e.g.
``channel``) in ``accepts[]`` no longer breaks deployed paid calls at parse
time. The direct per-entry parser (``PaymentAccept.from_dict``) stays strict,
and scheme selection stays fail-closed (skip is never select). This smoke
pins that tolerance against the live fixture shape.
"""

from __future__ import annotations

import copy
import json
from pathlib import Path

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
    assert pr.unrecognized_schemes == []


def test_live_402_fixture_with_channel_scheme_parses_and_keeps_exact() -> None:
    data = _load_fixture()
    channel_entry = copy.deepcopy(data["accepts"][0])
    channel_entry["scheme"] = "channel"
    data["accepts"].append(channel_entry)
    # Slice B tolerance: the unknown-scheme advert is skipped-and-represented;
    # the known entries survive byte-identically for fail-closed selection.
    pr = PaymentRequired.from_dict(data)
    original = _load_fixture()["accepts"]
    assert [a.scheme for a in pr.accepts] == [a["scheme"] for a in original]
    assert pr.unrecognized_schemes == ["channel"]
