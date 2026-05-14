"""Shared types and helpers for audit-docs checks.

Every check imports `Finding`, `Severity`, `AuditContext` from here. Keep this
file dependency-free (stdlib only) so a fresh CI runner can execute the audit
without an install step.
"""

from __future__ import annotations

import json
import re
import subprocess
import sys
from dataclasses import dataclass, field, asdict
from enum import Enum
from pathlib import Path


class Severity(str, Enum):
    ERROR = "error"
    WARNING = "warning"
    INFO = "info"


@dataclass(frozen=True)
class Finding:
    check: str
    severity: Severity
    message: str
    file: str | None = None
    line: int | None = None
    suggestion: str | None = None

    def to_dict(self) -> dict:
        d = asdict(self)
        d["severity"] = self.severity.value
        return d


@dataclass
class AuditContext:
    """Inputs shared across checks. Built once by audit.py."""

    repo_root: Path
    cli_binary: Path | None = None
    doc_files: list[Path] = field(default_factory=list)
    source_files: list[Path] = field(default_factory=list)
    sdk_source_files: list[Path] = field(default_factory=list)

    def relpath(self, p: Path) -> str:
        try:
            return str(p.relative_to(self.repo_root))
        except ValueError:
            return str(p)


# ---------------------------------------------------------------------------
# Markdown / MDX helpers
# ---------------------------------------------------------------------------

_FENCE_RE = re.compile(
    r"^([ \t]*)```([\w-]*)[^\n]*\n(.*?)^\1```",
    re.MULTILINE | re.DOTALL,
)


@dataclass(frozen=True)
class CodeBlock:
    file: Path
    lang: str
    body: str
    start_line: int

    def lines(self) -> list[tuple[int, str]]:
        out = []
        for i, line in enumerate(self.body.splitlines()):
            out.append((self.start_line + 1 + i, line))
        return out


def extract_code_blocks(file: Path, langs: set[str] | None = None) -> list[CodeBlock]:
    text = file.read_text(encoding="utf-8")
    blocks: list[CodeBlock] = []
    for m in _FENCE_RE.finditer(text):
        lang = m.group(2).lower()
        if langs is not None and lang not in langs:
            continue
        start_line = text.count("\n", 0, m.start()) + 1
        blocks.append(CodeBlock(file=file, lang=lang, body=m.group(3), start_line=start_line))
    return blocks


# ---------------------------------------------------------------------------
# File discovery
# ---------------------------------------------------------------------------

_DOC_IGNORE_DIRS = {
    ".git",
    "node_modules",
    "target",
    ".next",
    "dist",
    "build",
    "out",
    "coverage",
    ".turbo",
    ".vercel",
    "memory",
    ".local-plans",   # local planning notes, not shipped docs
    ".worktrees",     # secondary worktrees mirror the repo
    ".claude",        # plan/skill scaffolding under .claude/
}


def find_doc_files(root: Path) -> list[Path]:
    out: list[Path] = []
    for p in root.rglob("*"):
        if not p.is_file():
            continue
        if p.suffix not in {".md", ".mdx"}:
            continue
        if any(part in _DOC_IGNORE_DIRS for part in p.relative_to(root).parts):
            continue
        out.append(p)
    return sorted(out)


def find_rust_sources(root: Path) -> list[Path]:
    crates = root / "crates"
    if not crates.is_dir():
        return []
    return sorted(crates.rglob("*.rs"))


# Directories where SDK source lives. Each is scanned for env-var reads in its
# native language (Python: os.environ/getenv, TS: process.env, Go: os.Getenv).
_SDK_SOURCE_DIRS = ("python", "typescript", "go", "mcp", "openclaw-provider", "ai-sdk-provider")


_SDK_IGNORE_DIRS = {
    "node_modules",
    "dist",
    "build",
    ".next",
    "target",
    ".venv",
    "venv",
    "__pycache__",
    ".pytest_cache",
    "site-packages",
    "vendor",
    ".tox",
    ".mypy_cache",
    ".ruff_cache",
}


def find_sdk_sources(root: Path) -> list[Path]:
    """Files under sdks/* in languages we recognize for env-var scanning."""
    sdks = root / "sdks"
    if not sdks.is_dir():
        return []
    out: list[Path] = []
    for sdk in _SDK_SOURCE_DIRS:
        sdk_dir = sdks / sdk
        if not sdk_dir.is_dir():
            continue
        for ext in ("*.py", "*.ts", "*.tsx", "*.js", "*.mjs", "*.go"):
            for p in sdk_dir.rglob(ext):
                if any(part in _SDK_IGNORE_DIRS for part in p.parts):
                    continue
                out.append(p)
    return sorted(out)


# ---------------------------------------------------------------------------
# CLI introspection
# ---------------------------------------------------------------------------


def find_cli_binary(root: Path) -> Path | None:
    candidates = [
        root / "target" / "release" / "solvela",
        root / "target" / "debug" / "solvela",
    ]
    for c in candidates:
        if c.is_file():
            return c
    return None


def run_cli_help(binary: Path, args: list[str]) -> str:
    try:
        result = subprocess.run(
            [str(binary), *args, "--help"],
            capture_output=True,
            text=True,
            timeout=10,
        )
        return result.stdout
    except (subprocess.SubprocessError, OSError):
        return ""


_FLAG_RE = re.compile(r"(-{1,2}[\w-]+)(?:[=\s,]|$)")


def parse_help_flags(help_text: str) -> set[str]:
    flags: set[str] = set()
    for m in _FLAG_RE.finditer(help_text):
        flags.add(m.group(1))
    return flags


def parse_help_subcommands(help_text: str) -> set[str]:
    out: set[str] = set()
    in_commands = False
    for line in help_text.splitlines():
        if line.startswith("Commands:"):
            in_commands = True
            continue
        if in_commands:
            if not line.strip():
                continue
            if line.startswith("Options:") or line.startswith("Arguments:"):
                break
            m = re.match(r"^  ([a-z][\w-]+)\s+", line)
            if m:
                out.add(m.group(1))
    return out


# ---------------------------------------------------------------------------
# Output formatting
# ---------------------------------------------------------------------------


def format_findings_text(findings: list[Finding], repo_root: Path) -> str:
    if not findings:
        return "audit-docs: no findings.\n"

    by_file: dict[str, list[Finding]] = {}
    for f in findings:
        by_file.setdefault(f.file or "<global>", []).append(f)

    out_lines: list[str] = []
    for fname in sorted(by_file.keys()):
        out_lines.append(f"\n{fname}")
        for f in by_file[fname]:
            loc = f":{f.line}" if f.line is not None else ""
            out_lines.append(f"  [{f.severity.value}] {f.check}{loc}: {f.message}")
            if f.suggestion:
                out_lines.append(f"      suggestion: {f.suggestion}")

    errors = sum(1 for f in findings if f.severity == Severity.ERROR)
    warnings = sum(1 for f in findings if f.severity == Severity.WARNING)
    out_lines.append("")
    out_lines.append(
        f"audit-docs: {errors} error(s), {warnings} warning(s), {len(findings)} total."
    )
    return "\n".join(out_lines) + "\n"


def format_findings_json(findings: list[Finding]) -> str:
    return json.dumps([f.to_dict() for f in findings], indent=2) + "\n"


def eprint(*args, **kwargs) -> None:
    print(*args, file=sys.stderr, **kwargs)
