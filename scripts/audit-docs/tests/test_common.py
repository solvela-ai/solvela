"""Tests for the shared helpers in `common.py`.

Focus on the markdown extraction and CLI-help parsing — these are the moving
parts every check depends on. If they regress, the whole audit silently
under-reports.
"""

from __future__ import annotations

from pathlib import Path

from common import (
    extract_code_blocks,
    parse_help_flags,
    parse_help_subcommands,
)


def _write(tmp_path: Path, name: str, body: str) -> Path:
    p = tmp_path / name
    p.write_text(body, encoding="utf-8")
    return p


def test_extract_code_blocks_basic(tmp_path: Path) -> None:
    md = _write(
        tmp_path,
        "x.md",
        "intro\n\n```bash\nls -la\necho hi\n```\n\ntail\n",
    )
    blocks = extract_code_blocks(md)
    assert len(blocks) == 1
    assert blocks[0].lang == "bash"
    assert blocks[0].body.splitlines() == ["ls -la", "echo hi"]
    lines = blocks[0].lines()
    assert lines[0] == (4, "ls -la")
    assert lines[1] == (5, "echo hi")


def test_extract_code_blocks_lang_filter(tmp_path: Path) -> None:
    md = _write(
        tmp_path,
        "x.md",
        "```bash\nA\n```\n```rust\nfn x() {}\n```\n```\nplain\n```\n",
    )
    bash_only = extract_code_blocks(md, langs={"bash"})
    assert len(bash_only) == 1
    assert bash_only[0].lang == "bash"

    bash_or_empty = extract_code_blocks(md, langs={"bash", ""})
    assert len(bash_or_empty) == 2

    all_blocks = extract_code_blocks(md, langs=None)
    assert len(all_blocks) == 3


def test_extract_code_blocks_indented(tmp_path: Path) -> None:
    """Indented fences (e.g. inside MDX components) must close at the same indent."""
    md = _write(
        tmp_path,
        "x.md",
        "<Callout>\n  ```bash\n  echo nested\n  ```\n</Callout>\n",
    )
    blocks = extract_code_blocks(md)
    assert len(blocks) == 1
    assert blocks[0].lang == "bash"
    assert "echo nested" in blocks[0].body


def test_parse_help_flags() -> None:
    help_text = """Usage: solvela chat <PROMPT>

Options:
      --model <MODEL>     Model name [default: auto]
  -y, --yes               Skip prompt
      --scheme <SCHEME>   Force scheme
  -h, --help              Print help
"""
    flags = parse_help_flags(help_text)
    assert "--model" in flags
    assert "--yes" in flags
    assert "-y" in flags
    assert "--scheme" in flags
    assert "--help" in flags
    assert "-h" in flags
    assert "--nonexistent" not in flags


def test_parse_help_subcommands() -> None:
    help_text = """Usage: solvela [OPTIONS] <COMMAND>

Commands:
  wallet    Wallet management
  chat      Quick chat with auto-routing
  models    List available models and pricing
  health    Health check
  help      Print this message

Options:
      --api-url <API_URL>  Gateway URL
"""
    subs = parse_help_subcommands(help_text)
    assert subs == {"wallet", "chat", "models", "health", "help"}


def test_parse_help_subcommands_no_commands_section() -> None:
    """A leaf subcommand's --help has no Commands: section."""
    help_text = """Usage: solvela chat <PROMPT>

Options:
  --model <MODEL>
"""
    assert parse_help_subcommands(help_text) == set()
