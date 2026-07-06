#!/usr/bin/env python3
"""Scrape the gateway's /metrics and fail loudly when a money-safety signal trips.

Run by .github/workflows/metrics-alert.yml on a 15-minute cron; a non-zero exit
fails the workflow, which is the alert (GitHub emails the workflow author on
scheduled-run failures). Also runnable locally:

    SOLVELA_ADMIN_TOKEN=... python3 scripts/metrics_alert_check.py

Design notes:
- Counters are registered lazily by `metrics::counter!` — a counter that has
  never fired is ABSENT from the exposition. Absent means 0, not an error.
- Counters are monotonic, so raw `> 0` would alert forever after one incident.
  PAGE counters therefore compare against the committed, acknowledged baseline
  in ops/metrics-baseline.json: after investigating an incident, bump the
  baseline to the observed value in a normal PR (the ack is auditable history).
- Labeled counters are summed across their label sets.
- Two tiers: PAGE (exit 1) for settled-money-at-risk / custody signals;
  NOTICE (printed only) for signals with benign explanations (client retries).
"""

import json
import os
import re
import sys
import urllib.request

BASE = os.environ.get("SOLVELA_API_URL", "https://api.solvela.ai")
TOKEN = os.environ.get("SOLVELA_ADMIN_TOKEN")
BASELINE_PATH = os.path.join(os.path.dirname(__file__), "..", "ops", "metrics-baseline.json")

# Money-safety counters: any increase over the acknowledged baseline pages.
PAGE_COUNTERS = [
    # A2A settle path — funds settled but state/ledger/receipt at risk
    "solvela_a2a_task_stuck_working_total",
    "solvela_a2a_task_state_update_failed_after_settle_total",
    "solvela_a2a_settle_lock_release_failed_total",
    "solvela_a2a_provider_failed_after_settle_total",
    "solvela_a2a_settlement_record_skipped_total",
    # Ledger / receipt durability (all paid paths)
    "solvela_spend_log_skipped_total",
    "solvela_receipt_write_failures_total",
    "solvela_receipt_skipped_total",
    "solvela_vendor_receivable_write_failures_total",
    # Channel custody — refund worker must not silently degrade
    "solvela_channel_refund_sweep_error_total",
    "solvela_channel_refund_mark_accepted_failed_total",
    "solvela_channel_refund_claim_lost_total",
    "solvela_channel_refund_corrupt_row_total",
    "solvela_channel_refund_balance_insufficient_total",
    "solvela_channel_refund_daily_cap_exceeded_total",
    "solvela_channel_draw_lock_lost_total",
]

# Informational: worth eyes, but a benign explanation exists (e.g. client
# retries a completed task; replay of an already-used tx). Printed, never fails.
NOTICE_COUNTERS = [
    "solvela_a2a_settle_state_recheck_rejected_total",
    "solvela_replay_rejections_total",
    "solvela_a2a_cancel_total",
    "solvela_channel_draw_serve_timeout_total",
    "solvela_channel_refund_held_total",
]

# Gauges: instantaneous thresholds, no baseline needed.
# fee-payer floors mirror config/default.toml [monitor] critical/warn.
GAUGE_RULES = [
    ("solvela_fee_payer_balance_sol", "min", float(os.environ.get("ALERT_MIN_FEE_PAYER_SOL", "0.1"))),
    ("solvela_wallet_balance_sol", "min", float(os.environ.get("ALERT_MIN_WALLET_SOL", "0.02"))),
    ("solvela_channel_refund_held_count", "max", float(os.environ.get("ALERT_MAX_REFUNDS_HELD", "0"))),
    ("solvela_channel_refund_held_oldest_seconds", "max", float(os.environ.get("ALERT_MAX_HELD_AGE_SECS", "3600"))),
    ("solvela_channel_refund_oldest_pending_seconds", "max", float(os.environ.get("ALERT_MAX_PENDING_AGE_SECS", "3600"))),
]

LINE_RE = re.compile(r'^(?P<name>[a-zA-Z_:][a-zA-Z0-9_:]*)(?P<labels>\{[^}]*\})?\s+(?P<value>[-+0-9.eE]+|NaN)\s*$')


def scrape():
    if not TOKEN:
        print("FATAL: SOLVELA_ADMIN_TOKEN is not set", file=sys.stderr)
        sys.exit(2)
    req = urllib.request.Request(f"{BASE}/metrics", headers={"Authorization": f"Bearer {TOKEN}"})
    with urllib.request.urlopen(req, timeout=30) as resp:
        if resp.status != 200:
            print(f"FATAL: /metrics returned HTTP {resp.status}", file=sys.stderr)
            sys.exit(2)
        return resp.read().decode("utf-8", errors="replace")


def parse(text):
    """name -> summed value across label sets (absent counter == 0 by lookup default)."""
    values = {}
    for line in text.splitlines():
        if line.startswith("#"):
            continue
        m = LINE_RE.match(line.strip())
        if not m or m.group("value") == "NaN":
            continue
        values[m.group("name")] = values.get(m.group("name"), 0.0) + float(m.group("value"))
    return values


def main():
    with open(os.path.abspath(BASELINE_PATH)) as f:
        baseline = json.load(f)

    values = parse(scrape())
    failures, notices = [], []

    for name in PAGE_COUNTERS:
        observed, acked = values.get(name, 0.0), float(baseline.get(name, 0))
        if observed > acked:
            failures.append(f"PAGE  {name}: {observed:g} (acknowledged baseline {acked:g})")

    for name in NOTICE_COUNTERS:
        observed = values.get(name, 0.0)
        if observed > 0:
            notices.append(f"note  {name}: {observed:g}")

    for name, kind, limit in GAUGE_RULES:
        if name not in values:
            continue  # gauge not yet emitted (e.g. channel disabled) — nothing to judge
        v = values[name]
        if kind == "min" and v < limit:
            failures.append(f"PAGE  {name}: {v:g} below floor {limit:g}")
        if kind == "max" and v > limit:
            failures.append(f"PAGE  {name}: {v:g} above ceiling {limit:g}")

    for line in notices:
        print(line)
    if failures:
        print(f"\n{len(failures)} alert(s) tripped against {BASE}:", file=sys.stderr)
        for line in failures:
            print(line, file=sys.stderr)
        print("\nInvestigate, then acknowledge by bumping ops/metrics-baseline.json in a PR.", file=sys.stderr)
        sys.exit(1)
    print(f"OK — no money-safety alerts ({len(values)} series scraped from {BASE})")


if __name__ == "__main__":
    main()
