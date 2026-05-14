"""Tests for the numeric_claims check.

Build a tmp repo with mock source-of-truth files (Cargo.toml, package.json,
constants.rs, models.toml) and a couple of doc files with both correct and
drifting claims, then assert findings.
"""

from __future__ import annotations

from pathlib import Path

from common import AuditContext
from checks import numeric_claims


def _make_repo(
    tmp_path: Path,
    fee_percent: str = "5",
    sdk_version: str = "0.2.2",
    cli_version: str = "0.2.0",
    api_url: str = "https://api.solvela.ai",
) -> Path:
    (tmp_path / "Cargo.toml").write_text(
        f"[workspace.package]\nversion = \"{cli_version}\"\nedition = \"2021\"\n",
        encoding="utf-8",
    )
    (tmp_path / "crates" / "protocol" / "src").mkdir(parents=True)
    (tmp_path / "crates" / "protocol" / "src" / "constants.rs").write_text(
        f"pub const PLATFORM_FEE_PERCENT: u8 = {fee_percent};\n",
        encoding="utf-8",
    )
    (tmp_path / "sdks" / "typescript").mkdir(parents=True)
    (tmp_path / "sdks" / "typescript" / "package.json").write_text(
        f'{{"name":"@solvela/sdk","version":"{sdk_version}"}}\n',
        encoding="utf-8",
    )
    (tmp_path / "crates" / "cli" / "src").mkdir(parents=True)
    (tmp_path / "crates" / "cli" / "src" / "main.rs").write_text(
        f'#[arg(default_value = "{api_url}")]\npub api_url: String;\n',
        encoding="utf-8",
    )
    return tmp_path


def _ctx(tmp_path: Path, doc_text: str, doc_name: str = "x.md") -> AuditContext:
    docs_dir = tmp_path / "docs"
    docs_dir.mkdir(exist_ok=True)
    p = docs_dir / doc_name
    p.write_text(doc_text, encoding="utf-8")
    return AuditContext(repo_root=tmp_path, doc_files=[p])


def test_fee_percent_matches(tmp_path: Path) -> None:
    _make_repo(tmp_path)
    ctx = _ctx(tmp_path, "We charge a 5% platform fee per request.")
    findings = numeric_claims.run(ctx)
    assert findings == []


def test_fee_percent_drift_is_error(tmp_path: Path) -> None:
    _make_repo(tmp_path)
    ctx = _ctx(tmp_path, "The 3% platform fee covers infrastructure.")
    findings = numeric_claims.run(ctx)
    assert len(findings) == 1
    assert "3" in findings[0].message and "5" in findings[0].message


def test_sdk_version_drift_is_error(tmp_path: Path) -> None:
    _make_repo(tmp_path, sdk_version="0.2.2")
    ctx = _ctx(tmp_path, "Install via `@solvela/sdk@0.2.1`.")
    findings = numeric_claims.run(ctx)
    assert any("0.2.1" in f.message for f in findings)


def test_changelog_is_exempt(tmp_path: Path) -> None:
    """Historical version mentions in CHANGELOG.md must not be flagged."""
    _make_repo(tmp_path, sdk_version="0.2.2")
    ctx = _ctx(
        tmp_path,
        "Released 2026-04-01: `@solvela/sdk@0.1.0`.",
        doc_name="CHANGELOG.md",
    )
    findings = numeric_claims.run(ctx)
    assert findings == []


def test_status_md_is_exempt(tmp_path: Path) -> None:
    """STATUS.md interleaves current and historical claims; skip it for now."""
    _make_repo(tmp_path, sdk_version="0.2.2")
    ctx = _ctx(
        tmp_path,
        "The previous `@solvela/sdk@0.2.1` was wire-incompatible.",
        doc_name="STATUS.md",
    )
    findings = numeric_claims.run(ctx)
    assert findings == []
