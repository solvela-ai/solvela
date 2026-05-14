"""Tests for the links check.

Internal + anchor links are deterministic; we don't exercise the external HEAD
path here (network-dependent — that's covered by integration smoke tests
behind --check-external).
"""

from __future__ import annotations

from pathlib import Path

from common import AuditContext
from checks import links
from checks.links import _slugify


def _docs(tmp_path: Path, files: dict[str, str]) -> AuditContext:
    docs_dir = tmp_path / "docs"
    docs_dir.mkdir(parents=True, exist_ok=True)
    doc_paths: list[Path] = []
    for name, body in files.items():
        p = docs_dir / name
        p.write_text(body, encoding="utf-8")
        doc_paths.append(p)
    return AuditContext(repo_root=tmp_path, doc_files=doc_paths)


def test_internal_link_resolves(tmp_path: Path) -> None:
    ctx = _docs(
        tmp_path,
        {
            "a.md": "See [b](./b.md) for details.",
            "b.md": "# B\nbody",
        },
    )
    findings = links.run(ctx)
    assert findings == []


def test_broken_internal_link_is_error(tmp_path: Path) -> None:
    ctx = _docs(tmp_path, {"a.md": "See [missing](./does-not-exist.md)."})
    findings = links.run(ctx)
    assert len(findings) == 1
    assert findings[0].severity.value == "error"
    assert "does-not-exist" in findings[0].message


def test_anchor_link_resolves(tmp_path: Path) -> None:
    ctx = _docs(
        tmp_path,
        {"a.md": "# Title\n\n## My Section\n\nContent.\n\n[See it](#my-section)"},
    )
    findings = links.run(ctx)
    assert findings == []


def test_broken_anchor_link_is_error(tmp_path: Path) -> None:
    ctx = _docs(
        tmp_path,
        {"a.md": "# Title\n\n## My Section\n\n[See it](#nonexistent)"},
    )
    findings = links.run(ctx)
    assert len(findings) == 1
    assert "#nonexistent" in findings[0].message


def test_absolute_doc_path_is_skipped(tmp_path: Path) -> None:
    """Fumadocs `/docs/sdks/rust` paths are routing-relative, not file-relative;
    we don't validate them against the filesystem."""
    ctx = _docs(tmp_path, {"a.md": "See [rust](/docs/sdks/rust)."})
    findings = links.run(ctx)
    assert findings == []


def test_external_links_skipped_by_default(tmp_path: Path) -> None:
    ctx = _docs(tmp_path, {"a.md": "See [example](https://example.com)."})
    findings = links.run(ctx)
    assert findings == []


def test_mailto_and_tel_skipped(tmp_path: Path) -> None:
    ctx = _docs(
        tmp_path, {"a.md": "[mail](mailto:a@b.com) and [tel](tel:+15551234)"}
    )
    findings = links.run(ctx)
    assert findings == []


def test_slugify_basic() -> None:
    assert _slugify("My Section") == "my-section"
    assert _slugify("Multi   Spaces") == "multi-spaces"
    assert _slugify("Punctuation, here!") == "punctuation-here"
    assert _slugify("snake_case") == "snake-case"
    assert _slugify("HTTP/2 support") == "http2-support"
