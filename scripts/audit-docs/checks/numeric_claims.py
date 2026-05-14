"""Check 4: curated numeric and version claims match their source of truth.

Each claim is (pattern, extractor) — a regex that matches doc text and a
function that pulls the canonical value from a source file. When the doc
captures a value that doesn't match the canonical, emit an error.

Add new claims by appending to `CLAIMS` below. Keep the list short and
high-signal: every claim here costs a small amount of audit time, and
overly-aggressive patterns produce false positives that train reviewers
to ignore the audit.
"""

from __future__ import annotations

import json
import re
from dataclasses import dataclass
from pathlib import Path
from typing import Callable

from common import (
    AuditContext,
    Finding,
    Severity,
)


CHECK_NAME = "numeric_claims"


@dataclass(frozen=True)
class Claim:
    name: str
    pattern: re.Pattern[str]
    extractor: Callable[[Path], str | None]
    source_hint: str


def run(ctx: AuditContext) -> list[Finding]:
    canonical: dict[str, str | None] = {}
    extractor_findings: list[Finding] = []
    for claim in CLAIMS:
        try:
            val = claim.extractor(ctx.repo_root)
        except Exception as exc:
            extractor_findings.append(
                Finding(
                    check=CHECK_NAME,
                    severity=Severity.WARNING,
                    message=(
                        f"could not extract canonical value for `{claim.name}`: {exc}. "
                        f"Source: {claim.source_hint}. Doc claims skipped for this run."
                    ),
                )
            )
            val = None
        canonical[claim.name] = val

    findings: list[Finding] = list(extractor_findings)

    for doc in ctx.doc_files:
        # Changelogs, rolling-status files, and historical-record files document
        # past versions alongside current state. Auto-flagging every occurrence
        # would force erasing history. Skip claim verification on these — a
        # smarter context-aware check (Phase 2 / agentic) can revisit.
        if doc.name.lower() in {
            "changelog.md",
            "history.md",
            "releases.md",
            "status.md",
        }:
            continue
        try:
            text = doc.read_text(encoding="utf-8")
        except (OSError, UnicodeDecodeError):
            continue
        for claim in CLAIMS:
            truth = canonical.get(claim.name)
            if truth is None:
                continue
            for m in claim.pattern.finditer(text):
                captured = m.group(1) if m.groups() else m.group(0)
                if captured != truth:
                    lineno = text.count("\n", 0, m.start()) + 1
                    findings.append(
                        Finding(
                            check=CHECK_NAME,
                            severity=Severity.ERROR,
                            message=(
                                f"`{claim.name}`: doc says `{captured}`, "
                                f"source-of-truth ({claim.source_hint}) says `{truth}`"
                            ),
                            file=ctx.relpath(doc),
                            line=lineno,
                            suggestion=f"update the doc value to `{truth}`",
                        )
                    )
    return findings


def _extract_fee_percent(root: Path) -> str | None:
    path = root / "crates/protocol/src/constants.rs"
    m = re.search(
        r"PLATFORM_FEE_PERCENT\s*:\s*u\d+\s*=\s*(\d+)\s*;",
        path.read_text(encoding="utf-8"),
    )
    return m.group(1) if m else None


def _extract_workspace_version(root: Path) -> str | None:
    text = (root / "Cargo.toml").read_text(encoding="utf-8")
    m = re.search(
        r"\[workspace\.package\][^\[]*?\nversion\s*=\s*\"([^\"]+)\"",
        text,
        re.DOTALL,
    )
    return m.group(1) if m else None


def _extract_ts_sdk_version(root: Path) -> str | None:
    path = root / "sdks/typescript/package.json"
    if not path.is_file():
        return None
    pkg = json.loads(path.read_text(encoding="utf-8"))
    v = pkg.get("version")
    return str(v) if v else None


def _extract_default_api_url(root: Path) -> str | None:
    text = (root / "crates/cli/src/main.rs").read_text(encoding="utf-8")
    m = re.search(
        r"default_value\s*=\s*\"(https?://[^\"]+)\"",
        text,
    )
    return m.group(1) if m else None


CLAIMS: list[Claim] = [
    Claim(
        name="platform_fee_percent",
        pattern=re.compile(r"(\d+)\s*%\s*(?:platform\s+)?fee", re.IGNORECASE),
        extractor=_extract_fee_percent,
        source_hint="crates/protocol/src/constants.rs:PLATFORM_FEE_PERCENT",
    ),
    Claim(
        name="ts_sdk_version",
        pattern=re.compile(r"@solvela/sdk@(\d+\.\d+\.\d+)"),
        extractor=_extract_ts_sdk_version,
        source_hint="sdks/typescript/package.json:version",
    ),
    Claim(
        name="cli_workspace_version",
        pattern=re.compile(r"solvela-cli\s+v?(\d+\.\d+\.\d+)"),
        extractor=_extract_workspace_version,
        source_hint="Cargo.toml:workspace.package.version",
    ),
    Claim(
        name="default_api_url",
        pattern=re.compile(r"default[s:\s]+(https?://api\.solvela\.[a-z]+)"),
        extractor=_extract_default_api_url,
        source_hint="crates/cli/src/main.rs:default_value",
    ),
]
