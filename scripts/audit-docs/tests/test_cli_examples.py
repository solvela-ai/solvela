"""Tests for the cli_examples check (the regression case for `--stream`).

These tests don't need a real solvela binary — we exercise the token
parser / slicing helpers directly, and use synthetic `--help` outputs
fed through a mock binary stub.
"""

from __future__ import annotations

from pathlib import Path

import pytest

from checks.cli_examples import (
    _audit_block,
    _flag_has_inline_value,
    _flag_name,
    _slice_invocation,
    _looks_like_positional_arg,
)
from common import AuditContext, CodeBlock


_TOP_FLAGS = {"--api-url", "-h", "--help", "-V", "--version"}
_SUBCOMMANDS = {"chat", "wallet", "models", "health", "help"}
_SUB_FLAGS = {
    "chat": {"--model", "-y", "--yes", "--scheme", "-h", "--help"},
    "wallet": {"-h", "--help"},
    "models": {"-h", "--help"},
    "health": {"-h", "--help"},
}
_SUB_SUBS = {
    "chat": set(),
    "wallet": {"init", "status", "export"},
    "models": set(),
    "health": set(),
}


def _block_from(tmp_path: Path, body: str) -> CodeBlock:
    p = tmp_path / "x.md"
    p.write_text(f"```bash\n{body}\n```\n", encoding="utf-8")
    return CodeBlock(file=p, lang="bash", body=body, start_line=1)


def _ctx(tmp_path: Path) -> AuditContext:
    return AuditContext(repo_root=tmp_path)


def test_real_command_passes(tmp_path: Path) -> None:
    block = _block_from(tmp_path, 'solvela chat --model gpt-4o "hi"')
    findings = _audit_block(
        block, _ctx(tmp_path), _TOP_FLAGS, _SUBCOMMANDS, _SUB_FLAGS, _SUB_SUBS
    )
    assert findings == []


def test_ghost_flag_is_error(tmp_path: Path) -> None:
    """`solvela chat --stream "x"` — the exact bug that prompted this audit."""
    block = _block_from(tmp_path, 'solvela chat --stream "Write a story."')
    findings = _audit_block(
        block, _ctx(tmp_path), _TOP_FLAGS, _SUBCOMMANDS, _SUB_FLAGS, _SUB_SUBS
    )
    errors = [f for f in findings if f.severity.value == "error"]
    assert len(errors) == 1
    assert "--stream" in errors[0].message


def test_unknown_subcommand_is_error(tmp_path: Path) -> None:
    block = _block_from(tmp_path, "solvela bogus")
    findings = _audit_block(
        block, _ctx(tmp_path), _TOP_FLAGS, _SUBCOMMANDS, _SUB_FLAGS, _SUB_SUBS
    )
    assert len(findings) == 1
    assert "bogus" in findings[0].message


def test_unknown_subsubcommand_is_error(tmp_path: Path) -> None:
    """`solvela wallet create` instead of `solvela wallet init`."""
    block = _block_from(tmp_path, "solvela wallet create")
    findings = _audit_block(
        block, _ctx(tmp_path), _TOP_FLAGS, _SUBCOMMANDS, _SUB_FLAGS, _SUB_SUBS
    )
    errors = [f for f in findings if f.severity.value == "error"]
    assert len(errors) == 1
    assert "create" in errors[0].message


def test_help_subcommand_allowed_implicitly(tmp_path: Path) -> None:
    block = _block_from(tmp_path, "solvela help chat")
    findings = _audit_block(
        block, _ctx(tmp_path), _TOP_FLAGS, _SUBCOMMANDS, _SUB_FLAGS, _SUB_SUBS
    )
    assert findings == []


def test_top_level_flag_before_subcommand(tmp_path: Path) -> None:
    block = _block_from(
        tmp_path, 'solvela --api-url http://localhost:8402 chat "test"'
    )
    findings = _audit_block(
        block, _ctx(tmp_path), _TOP_FLAGS, _SUBCOMMANDS, _SUB_FLAGS, _SUB_SUBS
    )
    assert findings == []


def test_inline_flag_value(tmp_path: Path) -> None:
    """`--model=gpt-4o` form should be parsed the same as `--model gpt-4o`."""
    block = _block_from(tmp_path, "solvela chat --model=gpt-4o hi")
    findings = _audit_block(
        block, _ctx(tmp_path), _TOP_FLAGS, _SUBCOMMANDS, _SUB_FLAGS, _SUB_SUBS
    )
    assert findings == []


def test_lines_without_solvela_are_skipped(tmp_path: Path) -> None:
    block = _block_from(tmp_path, "cargo build\necho done\nls -la")
    findings = _audit_block(
        block, _ctx(tmp_path), _TOP_FLAGS, _SUBCOMMANDS, _SUB_FLAGS, _SUB_SUBS
    )
    assert findings == []


def test_pipe_truncates_invocation(tmp_path: Path) -> None:
    """`solvela chat ... | jq ...` — the jq part shouldn't be parsed as
    flags of solvela chat."""
    block = _block_from(
        tmp_path, "solvela chat hello | jq --raw-output '.choices'"
    )
    findings = _audit_block(
        block, _ctx(tmp_path), _TOP_FLAGS, _SUBCOMMANDS, _SUB_FLAGS, _SUB_SUBS
    )
    assert all("--raw-output" not in f.message for f in findings)


def test_dollar_prompt_prefix(tmp_path: Path) -> None:
    block = _block_from(tmp_path, "$ solvela chat hello")
    findings = _audit_block(
        block, _ctx(tmp_path), _TOP_FLAGS, _SUBCOMMANDS, _SUB_FLAGS, _SUB_SUBS
    )
    assert findings == []


@pytest.mark.parametrize(
    "token,expected",
    [
        ("--model", "--model"),
        ("--model=foo", "--model"),
        ("-y", "-y"),
    ],
)
def test_flag_name(token: str, expected: str) -> None:
    assert _flag_name(token) == expected


def test_flag_has_inline_value() -> None:
    assert _flag_has_inline_value("--model=foo")
    assert not _flag_has_inline_value("--model")


def test_slice_invocation_stops_at_pipe() -> None:
    assert _slice_invocation("solvela chat foo | jq .x") == "solvela chat foo"


def test_slice_invocation_keeps_quoted_pipe() -> None:
    assert _slice_invocation('solvela chat "a | b"') == 'solvela chat "a | b"'


def test_slice_invocation_stops_at_comment() -> None:
    assert (
        _slice_invocation("solvela chat foo  # explanation")
        == "solvela chat foo"
    )


@pytest.mark.parametrize(
    "token,expected",
    [
        ("hello", False),
        ("hello world", True),
        ("/some/path", True),
        ("https://example.com", True),
        ("init", False),
        ("install", False),
    ],
)
def test_looks_like_positional_arg(token: str, expected: bool) -> None:
    assert _looks_like_positional_arg(token) is expected
