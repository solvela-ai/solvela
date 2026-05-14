"""Tests for the env_vars check.

Each test stands up a tiny synthetic project (a few .rs and .md files in a
tmpdir) and runs the check directly. We assert on the names and severities
of findings, not their exact wording.
"""

from __future__ import annotations

from pathlib import Path

from common import AuditContext, Severity
from checks import env_vars


def _ctx(
    tmp_path: Path,
    rust: dict[str, str] | None = None,
    docs: dict[str, str] | None = None,
    env_example: str | None = None,
) -> AuditContext:
    rust_dir = tmp_path / "crates" / "thing" / "src"
    rust_dir.mkdir(parents=True, exist_ok=True)
    rust_paths: list[Path] = []
    for name, body in (rust or {}).items():
        p = rust_dir / name
        p.write_text(body, encoding="utf-8")
        rust_paths.append(p)

    docs_dir = tmp_path / "docs"
    docs_dir.mkdir(parents=True, exist_ok=True)
    doc_paths: list[Path] = []
    for name, body in (docs or {}).items():
        p = docs_dir / name
        p.write_text(body, encoding="utf-8")
        doc_paths.append(p)

    if env_example is not None:
        (tmp_path / ".env.example").write_text(env_example, encoding="utf-8")

    return AuditContext(
        repo_root=tmp_path,
        doc_files=doc_paths,
        source_files=rust_paths,
    )


def test_env_read_in_code_but_not_documented_warns(tmp_path: Path) -> None:
    ctx = _ctx(
        tmp_path,
        rust={"main.rs": 'fn x() { std::env::var("SOLVELA_NEW_THING"); }'},
    )
    findings = env_vars.run(ctx)
    matching = [f for f in findings if "SOLVELA_NEW_THING" in f.message]
    assert len(matching) == 1
    assert matching[0].severity == Severity.WARNING


def test_env_documented_but_not_read_errors(tmp_path: Path) -> None:
    ctx = _ctx(
        tmp_path,
        rust={"main.rs": "// no env reads here"},
        docs={"x.md": "```bash\nexport SOLVELA_FAKE=value\n```\n"},
    )
    findings = env_vars.run(ctx)
    fake = [f for f in findings if "SOLVELA_FAKE" in f.message]
    assert len(fake) == 1
    assert fake[0].severity == Severity.ERROR


def test_env_with_fallback_helper_is_recognized(tmp_path: Path) -> None:
    """The project uses `env_with_fallback("X", "Y")` everywhere. The check
    must treat both names as read or every documented var of that shape would
    show as undocumented-by-code."""
    ctx = _ctx(
        tmp_path,
        rust={
            "main.rs": (
                'fn main() { let _ = env_with_fallback("SOLVELA_HOST", "RCR_HOST"); }'
            )
        },
        docs={"x.md": "```bash\nexport SOLVELA_HOST=0.0.0.0\n```\n"},
    )
    findings = env_vars.run(ctx)
    for f in findings:
        assert "SOLVELA_HOST" not in f.message and "RCR_HOST" not in f.message, (
            f"unexpected env_with_fallback finding: {f}"
        )


def test_clap_env_attribute_is_recognized(tmp_path: Path) -> None:
    """Clap reads env vars via `#[arg(env = "X")]` — the regex must catch this."""
    ctx = _ctx(
        tmp_path,
        rust={
            "main.rs": (
                '#[arg(short, long, env = "SOLVELA_API_URL", default_value = "x")]\n'
                "pub api_url: String,"
            )
        },
        docs={"x.md": "```bash\nexport SOLVELA_API_URL=https://api.solvela.ai\n```\n"},
    )
    findings = env_vars.run(ctx)
    assert all("SOLVELA_API_URL" not in f.message for f in findings)


def test_rcr_alias_treated_as_covered(tmp_path: Path) -> None:
    ctx = _ctx(
        tmp_path,
        rust={
            "main.rs": (
                'fn main() { let _ = std::env::var("RCR_FOO"); '
                'let _ = std::env::var("SOLVELA_FOO"); }'
            )
        },
        docs={"x.md": "```bash\nexport SOLVELA_FOO=value\n```\n"},
    )
    findings = env_vars.run(ctx)
    for f in findings:
        assert "RCR_FOO" not in f.message, (
            f"RCR_FOO should be covered by SOLVELA_FOO docs; got {f}"
        )


def test_env_example_counts_as_documentation(tmp_path: Path) -> None:
    ctx = _ctx(
        tmp_path,
        rust={"main.rs": 'fn x() { std::env::var("SOLVELA_THING"); }'},
        env_example="SOLVELA_THING=example-value\n",
    )
    findings = env_vars.run(ctx)
    assert all("SOLVELA_THING" not in f.message for f in findings)


def test_short_doc_vars_are_ignored(tmp_path: Path) -> None:
    """Shell locals like SRC= and DST= in runbooks should not trigger the
    doc->code error path (they're not really env vars)."""
    ctx = _ctx(
        tmp_path,
        docs={"x.md": "```bash\nSRC=/tmp/a\nDST=/tmp/b\ncp $SRC $DST\n```\n"},
    )
    findings = env_vars.run(ctx)
    short = [f for f in findings if "`SRC`" in f.message or "`DST`" in f.message]
    assert short == []


def test_test_only_files_dont_require_docs(tmp_path: Path) -> None:
    rust_tests_dir = tmp_path / "crates" / "thing" / "tests"
    rust_tests_dir.mkdir(parents=True, exist_ok=True)
    test_file = rust_tests_dir / "smoke.rs"
    test_file.write_text(
        'fn t() { std::env::var("SOLVELA_TEST_ONLY_VAR"); }',
        encoding="utf-8",
    )
    ctx = AuditContext(repo_root=tmp_path, source_files=[test_file])
    findings = env_vars.run(ctx)
    assert all("SOLVELA_TEST_ONLY_VAR" not in f.message for f in findings)


def test_sdk_python_env_read_is_recognized(tmp_path: Path) -> None:
    sdk_py_dir = tmp_path / "sdks" / "python" / "src"
    sdk_py_dir.mkdir(parents=True, exist_ok=True)
    py = sdk_py_dir / "client.py"
    py.write_text(
        'import os\nx = os.environ.get("SOLVELA_LIVE_TESTS")\n',
        encoding="utf-8",
    )

    docs_dir = tmp_path / "docs"
    docs_dir.mkdir(parents=True, exist_ok=True)
    doc = docs_dir / "x.md"
    doc.write_text(
        "```bash\nexport SOLVELA_LIVE_TESTS=1\n```\n",
        encoding="utf-8",
    )

    ctx = AuditContext(
        repo_root=tmp_path,
        doc_files=[doc],
        sdk_source_files=[py],
    )
    findings = env_vars.run(ctx)
    assert all("SOLVELA_LIVE_TESTS" not in f.message for f in findings)
