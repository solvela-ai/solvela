"""Check 1: CLI examples in docs match the real CLI surface.

For every `solvela …` line in a fenced bash block, verify:
  - the subcommand exists (matched against `solvela --help`)
  - every short and long flag exists in the subcommand's own `--help`
  - if a subsubcommand follows (e.g. `solvela mcp install`), it exists too

This is the check that catches `--stream` ghost flags and renamed subcommands.

The check needs a built `solvela` binary. If one isn't found under
`target/release/` or `target/debug/`, it emits a single warning and skips
(rather than blocking the whole audit). CI builds the binary first.
"""

from __future__ import annotations

import re
import shlex

from common import (
    AuditContext,
    CodeBlock,
    Finding,
    Severity,
    extract_code_blocks,
    parse_help_flags,
    parse_help_subcommands,
    run_cli_help,
)


CHECK_NAME = "cli_examples"


def run(ctx: AuditContext) -> list[Finding]:
    if ctx.cli_binary is None:
        return [
            Finding(
                check=CHECK_NAME,
                severity=Severity.WARNING,
                message=(
                    "no `solvela` binary found under target/{release,debug}/; "
                    "skipping CLI example validation. Build with "
                    "`cargo build --release -p solvela-cli` before running."
                ),
            )
        ]

    top_help = run_cli_help(ctx.cli_binary, [])
    if not top_help:
        return [
            Finding(
                check=CHECK_NAME,
                severity=Severity.ERROR,
                message=f"`{ctx.cli_binary} --help` produced no output; binary may be broken.",
            )
        ]

    top_flags = parse_help_flags(top_help)
    subcommands = parse_help_subcommands(top_help)
    subcommands.add("help")

    sub_flags: dict[str, set[str]] = {}
    sub_subs: dict[str, set[str]] = {}
    for sub in subcommands:
        if sub == "help":
            continue
        h = run_cli_help(ctx.cli_binary, [sub])
        sub_flags[sub] = parse_help_flags(h)
        sub_subs[sub] = parse_help_subcommands(h)

    findings: list[Finding] = []
    for doc in ctx.doc_files:
        for block in extract_code_blocks(doc, langs={"bash", "sh", "shell", ""}):
            findings.extend(
                _audit_block(block, ctx, top_flags, subcommands, sub_flags, sub_subs)
            )
    return findings


_PROMPT_PREFIX_RE = re.compile(r"^\s*[#$>]\s+")


def _audit_block(
    block: CodeBlock,
    ctx: AuditContext,
    top_flags: set[str],
    subcommands: set[str],
    sub_flags: dict[str, set[str]],
    sub_subs: dict[str, set[str]],
) -> list[Finding]:
    findings: list[Finding] = []
    for lineno, raw in block.lines():
        line = _PROMPT_PREFIX_RE.sub("", raw).strip()
        if not line:
            continue
        if not re.match(r"^(solvela|solvela-cli)(\s|$)", line):
            continue

        invocation = _slice_invocation(line)

        try:
            tokens = shlex.split(invocation, comments=False, posix=True)
        except ValueError:
            findings.append(
                Finding(
                    check=CHECK_NAME,
                    severity=Severity.WARNING,
                    message=f"could not parse `solvela` invocation: {invocation!r}",
                    file=ctx.relpath(block.file),
                    line=lineno,
                )
            )
            continue

        if not tokens or tokens[0] not in {"solvela", "solvela-cli"}:
            continue

        findings.extend(
            _audit_tokens(
                tokens, lineno, block, ctx, top_flags, subcommands, sub_flags, sub_subs
            )
        )
    return findings


def _slice_invocation(line: str) -> str:
    """Return the substring up to the first shell operator at a token boundary.

    Keeps quoted regions intact (a `#` inside quotes is not a comment).
    """
    out: list[str] = []
    i = 0
    quote: str | None = None
    while i < len(line):
        c = line[i]
        if quote:
            out.append(c)
            if c == "\\" and i + 1 < len(line):
                out.append(line[i + 1])
                i += 2
                continue
            if c == quote:
                quote = None
            i += 1
            continue
        if c in ('"', "'"):
            quote = c
            out.append(c)
            i += 1
            continue
        if c in "|;&" and (i == 0 or line[i - 1].isspace() or c == ";"):
            break
        if c == ">" and (i == 0 or line[i - 1].isspace()):
            break
        if c == "#" and (i == 0 or line[i - 1].isspace()):
            break
        out.append(c)
        i += 1
    return "".join(out).strip()


def _audit_tokens(
    tokens: list[str],
    lineno: int,
    block: CodeBlock,
    ctx: AuditContext,
    top_flags: set[str],
    subcommands: set[str],
    sub_flags: dict[str, set[str]],
    sub_subs: dict[str, set[str]],
) -> list[Finding]:
    findings: list[Finding] = []
    file = ctx.relpath(block.file)

    i = 1
    saw_subcommand: str | None = None
    saw_subsub: str | None = None

    while i < len(tokens):
        t = tokens[i]
        if t.startswith("-"):
            flag = _flag_name(t)
            if flag not in top_flags and flag not in {"-h", "--help", "-V", "--version"}:
                findings.append(
                    Finding(
                        check=CHECK_NAME,
                        severity=Severity.ERROR,
                        message=f"unknown top-level flag `{flag}` on `solvela`",
                        file=file,
                        line=lineno,
                        suggestion=f"valid top-level flags: {sorted(top_flags)}",
                    )
                )
            if not _flag_has_inline_value(t):
                if (
                    i + 1 < len(tokens)
                    and not tokens[i + 1].startswith("-")
                    and tokens[i + 1] not in subcommands
                ):
                    i += 1
            i += 1
            continue
        if t in subcommands:
            saw_subcommand = t
            i += 1
            break
        findings.append(
            Finding(
                check=CHECK_NAME,
                severity=Severity.ERROR,
                message=f"unknown subcommand `{t}` (not in `solvela --help`)",
                file=file,
                line=lineno,
                suggestion=f"valid subcommands: {sorted(s for s in subcommands if s != 'help')}",
            )
        )
        return findings

    if saw_subcommand is None or saw_subcommand == "help":
        return findings

    valid_subs = sub_subs.get(saw_subcommand, set())
    if i < len(tokens) and not tokens[i].startswith("-"):
        candidate = tokens[i]
        if valid_subs and candidate in valid_subs:
            saw_subsub = candidate
            i += 1
        elif valid_subs and not _looks_like_positional_arg(candidate):
            findings.append(
                Finding(
                    check=CHECK_NAME,
                    severity=Severity.ERROR,
                    message=(
                        f"unknown subsubcommand `{candidate}` for `solvela {saw_subcommand}`"
                    ),
                    file=file,
                    line=lineno,
                    suggestion=f"valid subsubcommands: {sorted(valid_subs)}",
                )
            )

    valid_flags = set(sub_flags.get(saw_subcommand, set()))
    valid_flags |= top_flags

    while i < len(tokens):
        t = tokens[i]
        if t.startswith("-"):
            flag = _flag_name(t)
            if flag in {"-h", "--help", "-V", "--version"}:
                i += 1
                continue
            if flag not in valid_flags:
                sev = Severity.WARNING if saw_subsub else Severity.ERROR
                findings.append(
                    Finding(
                        check=CHECK_NAME,
                        severity=sev,
                        message=(
                            f"unknown flag `{flag}` on "
                            f"`solvela {saw_subcommand}{' ' + saw_subsub if saw_subsub else ''}`"
                        ),
                        file=file,
                        line=lineno,
                        suggestion=(
                            f"valid flags: {sorted(valid_flags)}"
                            if not saw_subsub
                            else None
                        ),
                    )
                )
            if not _flag_has_inline_value(t):
                if i + 1 < len(tokens) and not tokens[i + 1].startswith("-"):
                    i += 1
        i += 1

    return findings


def _flag_name(token: str) -> str:
    return token.split("=", 1)[0]


def _flag_has_inline_value(token: str) -> bool:
    return "=" in token


def _looks_like_positional_arg(token: str) -> bool:
    if not token:
        return True
    if any(c in token for c in " /.:\"'@$"):
        return True
    if not re.match(r"^[a-z][a-z0-9-]*$", token):
        return True
    return False
