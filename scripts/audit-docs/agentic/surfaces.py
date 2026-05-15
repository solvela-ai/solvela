"""Curated doc corpus + source-of-truth context gathering.

The agentic page-review check is opt-in and expensive, so we scope it
narrowly: only user-facing docs that ship as the public reading experience.
Internal runbooks, planning notes, and per-release operational docs are
excluded — they have a different audience and audit shape.

Add or remove entries by editing `_USER_FACING_PATTERNS` below. New entries
should match `pathlib.Path.match()` glob semantics relative to repo root.
"""

from __future__ import annotations

import fnmatch
from dataclasses import dataclass
from pathlib import Path

from common import (
    find_cli_binary,
    parse_help_subcommands,
    run_cli_help,
)


_USER_FACING_PATTERNS: tuple[str, ...] = (
    "README.md",
    "SECURITY.md",
    "BACKERS.md",
    "crates/cli/README.md",
    "sdks/typescript/README.md",
    "sdks/python/README.md",
    "sdks/go/README.md",
    "sdks/mcp/README.md",
    "sdks/cli-npm/README.md",
    "sdks/openclaw-provider/README.md",
    "sdks/ai-sdk-provider/README.md",
    "sdks/signer-core/README.md",
    "integrations/openclaw/README.md",
    "integrations/elizaos/README.md",
    "dashboard/content/docs/index.mdx",
    "dashboard/content/docs/quickstart.mdx",
    "dashboard/content/docs/sdks/*.mdx",
    "dashboard/content/docs/api/*.mdx",
    "dashboard/content/docs/concepts/*.mdx",
    "dashboard/content/docs/operations/*.mdx",
)

_EXCLUDE_PATTERNS: tuple[str, ...] = (
    "**/CLAUDE.md",
    "**/AGENTS.md",
    "STATUS.md",
    "CHANGELOG.md",
    "HANDOFF.md",
)


@dataclass(frozen=True)
class Doc:
    path: Path
    relpath: str
    text: str


def select_corpus(repo_root: Path, doc_filter: list[str] | None = None) -> list[Doc]:
    """Walk the curated patterns and return the matching files.

    `doc_filter`, if provided, narrows to docs whose relpath equals or
    starts with one of the supplied strings.
    """
    selected: list[Doc] = []
    seen: set[Path] = set()

    for pattern in _USER_FACING_PATTERNS:
        for path in repo_root.glob(pattern):
            if not path.is_file():
                continue
            if path in seen:
                continue
            relpath = path.relative_to(repo_root).as_posix()
            if _is_excluded(relpath):
                continue
            if doc_filter and not any(
                relpath == f or relpath.startswith(f.rstrip("/") + "/")
                for f in doc_filter
            ):
                continue
            try:
                text = path.read_text(encoding="utf-8")
            except (OSError, UnicodeDecodeError):
                continue
            selected.append(Doc(path=path, relpath=relpath, text=text))
            seen.add(path)

    selected.sort(key=lambda d: d.relpath)
    return selected


def _is_excluded(relpath: str) -> bool:
    for pat in _EXCLUDE_PATTERNS:
        if fnmatch.fnmatch(relpath, pat):
            return True
    return False


# Files bundled into every agent prompt as context the model can verify
# claims against. Keep this tight: every byte here is input tokens we pay
# for on every page. None = full file; integer = head N lines.
_SOT_FILES: tuple[tuple[str, int | None], ...] = (
    ("Cargo.toml", 30),
    ("crates/protocol/src/constants.rs", None),
    ("config/models.toml", None),
    ("crates/cli/src/main.rs", 130),
    ("sdks/typescript/package.json", None),
    (".env.example", None),
)


@dataclass(frozen=True)
class SourceContext:
    relpath: str
    excerpt: str
    note: str


def gather_context(repo_root: Path) -> list[SourceContext]:
    """Read the curated source-of-truth files. Skips missing files silently
    so the audit still works in worktrees that lack one or two paths."""
    out: list[SourceContext] = []
    for relpath, head_lines in _SOT_FILES:
        path = repo_root / relpath
        if not path.is_file():
            continue
        try:
            text = path.read_text(encoding="utf-8")
        except (OSError, UnicodeDecodeError):
            continue
        if head_lines is not None:
            text = "\n".join(text.splitlines()[:head_lines])
        out.append(
            SourceContext(
                relpath=relpath,
                excerpt=text,
                note=_SOT_NOTES.get(relpath, ""),
            )
        )

    cli = find_cli_binary(repo_root)
    if cli is not None:
        top_help = run_cli_help(cli, [])
        if top_help:
            out.append(
                SourceContext(
                    relpath="solvela --help",
                    excerpt=top_help,
                    note="top-level CLI help; lists real subcommands and global flags",
                )
            )
            for sub in sorted(parse_help_subcommands(top_help)):
                if sub == "help":
                    continue
                sub_help = run_cli_help(cli, [sub])
                if sub_help:
                    out.append(
                        SourceContext(
                            relpath=f"solvela {sub} --help",
                            excerpt=sub_help,
                            note=f"`solvela {sub}` help; real flags and subsubcommands",
                        )
                    )
    return out


_SOT_NOTES: dict[str, str] = {
    "Cargo.toml": (
        "workspace.package version is the source-of-truth for any CLI / "
        "library version mentioned in the docs"
    ),
    "crates/protocol/src/constants.rs": (
        "PLATFORM_FEE_PERCENT lives here — docs claiming a fee % must match"
    ),
    "config/models.toml": (
        "model registry; provider / pricing / capability claims are anchored here"
    ),
    "crates/cli/src/main.rs": (
        "Cli + Commands enum: real subcommand names, real flags, real default values"
    ),
    "sdks/typescript/package.json": "@solvela/sdk current published version",
    ".env.example": (
        "canonical list of user-facing env vars; docs that reference an env "
        "var should either include it here or list it in the SDK/CLI doc page"
    ),
}
