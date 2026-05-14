"""Check 6: link health — internal, anchor, and (optionally) external.

  - **Internal**: `[text](relative/or/absolute/path)` resolves to a real file
    in the repo. For `.md` / `.mdx` targets, also verifies anchor fragments
    against actual headings.
  - **Anchor (in-page)**: `[text](#section)` — verify the slug matches a
    heading in the same file.
  - **External**: `[text](https://…)` — HEAD with a short timeout. Network
    failures are reported as warnings, not errors (external flakes shouldn't
    block CI). Disabled by default; enable with `--check-external` on the CLI.

MDX-specific tags like `<a href="...">` are ignored — markdown link syntax
is the only thing we audit, so JSX with dynamic props doesn't produce noise.
"""

from __future__ import annotations

import re
from pathlib import Path
from urllib.error import URLError
from urllib.request import Request, urlopen

from common import (
    AuditContext,
    Finding,
    Severity,
)


CHECK_NAME = "links"


_MD_LINK_RE = re.compile(
    r"\[([^\]\n]+)\]\(([^)\s]+)(?:\s+\"[^\"]*\")?\)",
)
_HEADING_RE = re.compile(r"^#{1,6}\s+(.+?)\s*$", re.MULTILINE)

_EXTERNAL_FLAKY_HOSTS: frozenset[str] = frozenset({
    "twitter.com",
    "x.com",
    "linkedin.com",
})


def run(ctx: AuditContext, check_external: bool = False) -> list[Finding]:
    findings: list[Finding] = []

    anchor_map: dict[Path, set[str]] = {}
    for doc in ctx.doc_files:
        try:
            text = doc.read_text(encoding="utf-8")
        except (OSError, UnicodeDecodeError):
            continue
        slugs: set[str] = set()
        for m in _HEADING_RE.finditer(text):
            slugs.add(_slugify(m.group(1)))
        anchor_map[doc] = slugs

    for doc in ctx.doc_files:
        try:
            text = doc.read_text(encoding="utf-8")
        except (OSError, UnicodeDecodeError):
            continue
        for m in _MD_LINK_RE.finditer(text):
            target = m.group(2).strip()
            lineno = text.count("\n", 0, m.start()) + 1
            findings.extend(
                _audit_link(doc, target, lineno, ctx, anchor_map, check_external)
            )
    return findings


def _audit_link(
    source_doc: Path,
    target: str,
    lineno: int,
    ctx: AuditContext,
    anchor_map: dict[Path, set[str]],
    check_external: bool,
) -> list[Finding]:
    if target.startswith(("mailto:", "tel:")):
        return []
    if target.startswith("//"):
        return []
    if target.startswith(("http://", "https://")):
        if not check_external:
            return []
        return _audit_external(source_doc, target, lineno, ctx)
    if target.startswith("#"):
        return _audit_anchor(source_doc, target, lineno, ctx, anchor_map)
    return _audit_internal(source_doc, target, lineno, ctx, anchor_map)


def _audit_anchor(
    source_doc: Path,
    target: str,
    lineno: int,
    ctx: AuditContext,
    anchor_map: dict[Path, set[str]],
) -> list[Finding]:
    slug = target.lstrip("#")
    if not slug:
        return []
    slugs = anchor_map.get(source_doc, set())
    if slug not in slugs:
        return [
            Finding(
                check=CHECK_NAME,
                severity=Severity.ERROR,
                message=f"anchor `{target}` does not match any heading in this file",
                file=ctx.relpath(source_doc),
                line=lineno,
                suggestion=(
                    f"existing headings: {sorted(slugs)[:8]}{'…' if len(slugs) > 8 else ''}"
                ),
            )
        ]
    return []


def _audit_internal(
    source_doc: Path,
    target: str,
    lineno: int,
    ctx: AuditContext,
    anchor_map: dict[Path, set[str]],
) -> list[Finding]:
    path_part, _, anchor = target.partition("#")
    if not path_part:
        return []

    if path_part.startswith("/"):
        # Doc-relative absolute paths (Fumadocs convention): `/docs/sdks/rust`
        # resolves at routing time, not against the file tree. Skip.
        return []

    candidate = (source_doc.parent / path_part).resolve()
    if not candidate.exists():
        for ext in (".md", ".mdx"):
            with_ext = candidate.with_suffix(ext)
            if with_ext.exists():
                candidate = with_ext
                break
        else:
            return [
                Finding(
                    check=CHECK_NAME,
                    severity=Severity.ERROR,
                    message=f"internal link target `{target}` does not exist",
                    file=ctx.relpath(source_doc),
                    line=lineno,
                )
            ]

    if anchor and candidate.suffix in {".md", ".mdx"}:
        slugs = anchor_map.get(candidate, set())
        if slugs and anchor not in slugs:
            return [
                Finding(
                    check=CHECK_NAME,
                    severity=Severity.WARNING,
                    message=(
                        f"anchor `#{anchor}` in link `{target}` does not match any heading "
                        f"in {ctx.relpath(candidate)}"
                    ),
                    file=ctx.relpath(source_doc),
                    line=lineno,
                )
            ]
    return []


def _audit_external(
    source_doc: Path,
    target: str,
    lineno: int,
    ctx: AuditContext,
) -> list[Finding]:
    host = _host_of(target)
    is_flaky = host in _EXTERNAL_FLAKY_HOSTS
    try:
        req = Request(
            target,
            method="HEAD",
            headers={"User-Agent": "solvela-audit-docs/1.0"},
        )
        with urlopen(req, timeout=10) as resp:
            status = resp.status
    except URLError as e:
        return [
            Finding(
                check=CHECK_NAME,
                severity=Severity.WARNING,
                message=f"external link {target} unreachable: {e.reason}",
                file=ctx.relpath(source_doc),
                line=lineno,
            )
        ]
    except Exception as e:
        return [
            Finding(
                check=CHECK_NAME,
                severity=Severity.WARNING,
                message=f"external link {target} check failed: {e}",
                file=ctx.relpath(source_doc),
                line=lineno,
            )
        ]

    if status >= 400:
        sev = Severity.WARNING if is_flaky else Severity.ERROR
        return [
            Finding(
                check=CHECK_NAME,
                severity=sev,
                message=f"external link {target} returned HTTP {status}",
                file=ctx.relpath(source_doc),
                line=lineno,
            )
        ]
    return []


def _host_of(url: str) -> str:
    m = re.match(r"https?://([^/]+)", url)
    return m.group(1).lower() if m else ""


def _slugify(text: str) -> str:
    """GitHub-style anchor slug. Lowercase, hyphens for spaces, strip
    everything but [a-z0-9-]."""
    out = text.lower()
    out = re.sub(r"[^\w\s-]", "", out)
    out = re.sub(r"[\s_]+", "-", out)
    out = re.sub(r"-+", "-", out).strip("-")
    return out
