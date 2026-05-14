"""Check 3: every CLI subcommand has a doc mention.

For each `Commands::X` variant in the CLI (parsed via `solvela --help`),
require at least one mention of `solvela <subcommand>` in a fenced shell
block somewhere under the doc set.

This catches the inverse of check 1: not "wrong flag documented" but
"command exists, nobody documents it." Useful when adding a new subcommand
and forgetting the doc PR.

Severity = warning. Undocumented surface area is a process miss, not a
correctness bug — but still worth surfacing for solo-dev workflows.
"""

from __future__ import annotations

import re

from common import (
    AuditContext,
    Finding,
    Severity,
    extract_code_blocks,
    parse_help_subcommands,
    run_cli_help,
)


CHECK_NAME = "subcommand_coverage"


def run(ctx: AuditContext) -> list[Finding]:
    if ctx.cli_binary is None:
        return [
            Finding(
                check=CHECK_NAME,
                severity=Severity.WARNING,
                message=(
                    "no `solvela` binary found; skipping subcommand coverage check. "
                    "Build with `cargo build --release -p solvela-cli`."
                ),
            )
        ]

    top_help = run_cli_help(ctx.cli_binary, [])
    if not top_help:
        return [
            Finding(
                check=CHECK_NAME,
                severity=Severity.ERROR,
                message="`solvela --help` produced no output",
            )
        ]

    subcommands = parse_help_subcommands(top_help)
    if not subcommands:
        return [
            Finding(
                check=CHECK_NAME,
                severity=Severity.ERROR,
                message="could not parse subcommands from `solvela --help`",
            )
        ]

    mentioned = _collect_mentions(ctx, subcommands)

    findings: list[Finding] = []
    for sub in sorted(subcommands):
        if sub not in mentioned:
            findings.append(
                Finding(
                    check=CHECK_NAME,
                    severity=Severity.WARNING,
                    message=f"subcommand `solvela {sub}` is not documented in any .md/.mdx file",
                    suggestion=(
                        f"add an example to dashboard/content/docs/sdks/rust.mdx "
                        f"or crates/cli/README.md showing `solvela {sub}` in use"
                    ),
                )
            )
    return findings


def _collect_mentions(ctx: AuditContext, subcommands: set[str]) -> set[str]:
    pattern = re.compile(
        r"(?<![\w-])(?:solvela|solvela-cli)\s+([a-z][\w-]+)",
    )
    seen: set[str] = set()
    for doc in ctx.doc_files:
        for block in extract_code_blocks(doc, langs={"bash", "sh", "shell", ""}):
            for _lineno, line in block.lines():
                for m in pattern.finditer(line):
                    name = m.group(1)
                    if name in subcommands:
                        seen.add(name)
    return seen
