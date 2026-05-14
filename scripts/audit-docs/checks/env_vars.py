"""Check 2: env vars are bidirectionally consistent between code and docs.

Two reports:
  - **code -> doc**: every env var read by `crates/` source (excluding OS/shell
    builtins and test-only vars) must be mentioned somewhere in the docs OR
    in `.env.example`. Severity = warning (omission, not breakage).
  - **doc -> code**: every `export FOO=` (or otherwise documented var) must
    appear in a real `env::var()` call somewhere in the codebase. Severity =
    error (this is the class of bug that says "set FOO" when FOO is read
    nowhere — exactly what bit the user on `SOLANA_RPC_URL`).

Legacy `RCR_*` aliases are treated as documented when their `SOLVELA_*`
canonical is documented.
"""

from __future__ import annotations

import re
from pathlib import Path

from common import (
    AuditContext,
    Finding,
    Severity,
    extract_code_blocks,
)


CHECK_NAME = "env_vars"


# OS/shell vars that never need user-facing docs.
_OS_ALLOWLIST: frozenset[str] = frozenset({
    "HOME",
    "USERPROFILE",
    "APPDATA",
    "PATH",
    "USER",
    "LOGNAME",
    "TMPDIR",
    "TEMP",
    "TMP",
    "SHELL",
    "PWD",
    "OLDPWD",
    "CARGO_HOME",
    "RUSTUP_HOME",
    "RUST_LOG",
    "RUST_BACKTRACE",
})

# Test-only vars that don't need user docs.
_TEST_ONLY_ALLOWLIST: frozenset[str] = frozenset({
    "TEST_DATABASE_URL",
})

# Vars documented as illustrative or as part of an external tool's config,
# where it's correct that they don't appear in solvela source.
_DOC_ONLY_ALLOWLIST: frozenset[str] = frozenset({
    # Docker Compose / Postgres image init vars — consumed by the postgres
    # container, not by Solvela.
    "POSTGRES_USER",
    "POSTGRES_PASSWORD",
    "POSTGRES_DB",
    # OpenSSL build-system overrides documented for Linux build instructions
    # — read by the OpenSSL crate build script, not by Solvela source.
    "OPENSSL_NO_PKG_CONFIG",
    "OPENSSL_LIB_DIR",
    "OPENSSL_INCLUDE_DIR",
})


# Rust env-read sites we recognize:
#   - direct: std::env::var("X"), std::env::var_os("X"), env::var(...)
#   - clap attribute: #[arg(env = "X", ...)] / #[arg(... env = "X")]
#   - project-internal helpers (capture string-literal args):
#     env_with_fallback("X", "Y"), read_env_var("X", "Y")
_ENV_VAR_RUST_RE = re.compile(
    r'env::var(?:_os)?\("([A-Z_][A-Z0-9_]*)"\)'
    r'|env\s*=\s*"([A-Z_][A-Z0-9_]*)"'
    r'|(?:env_with_fallback|read_env_var)\(\s*"([A-Z_][A-Z0-9_]*)"\s*,\s*"([A-Z_][A-Z0-9_]*)"'
)

# Per-language env-read patterns for SDK sources. Each yields the var name in
# group(1) or group(2). Patterns intentionally conservative — false negatives
# (a var we miss in source code) cause a noisy "documented but never read"
# warning, not a silent regression.
_ENV_VAR_SDK_RE: dict[str, re.Pattern[str]] = {
    ".py": re.compile(
        r'os\.environ(?:\.get)?\(\s*["\']([A-Z_][A-Z0-9_]*)["\']'
        r'|os\.getenv\(\s*["\']([A-Z_][A-Z0-9_]*)["\']'
    ),
    ".ts": re.compile(r'process\.env(?:\.([A-Z_][A-Z0-9_]*)|\[\s*["\']([A-Z_][A-Z0-9_]*)["\']\s*\])'),
    ".tsx": re.compile(r'process\.env(?:\.([A-Z_][A-Z0-9_]*)|\[\s*["\']([A-Z_][A-Z0-9_]*)["\']\s*\])'),
    ".js": re.compile(r'process\.env(?:\.([A-Z_][A-Z0-9_]*)|\[\s*["\']([A-Z_][A-Z0-9_]*)["\']\s*\])'),
    ".mjs": re.compile(r'process\.env(?:\.([A-Z_][A-Z0-9_]*)|\[\s*["\']([A-Z_][A-Z0-9_]*)["\']\s*\])'),
    ".go": re.compile(
        r'os\.Getenv\(\s*"([A-Z_][A-Z0-9_]*)"|os\.LookupEnv\(\s*"([A-Z_][A-Z0-9_]*)"'
    ),
}

# Doc env var pattern: require min 4 chars and (a known prefix OR an `export `
# preface). This filters out shell locals like SRC=, DST=, OK= without
# allowlisting every runbook one-off.
_ENV_VAR_DOC_RE = re.compile(
    r'(?:^|\s)(?:export\s+([A-Z][A-Z0-9_]{3,})=|'
    r'(SOLVELA_[A-Z0-9_]+|RCR_[A-Z0-9_]+|SOLANA_[A-Z0-9_]+|'
    r'ANTHROPIC_[A-Z0-9_]+|OPENAI_[A-Z0-9_]+|GOOGLE_[A-Z0-9_]+|XAI_[A-Z0-9_]+|'
    r'DEEPSEEK_[A-Z0-9_]+|DATABASE_URL|REDIS_URL|POSTGRES_[A-Z0-9_]+)=)',
    re.MULTILINE,
)


def run(ctx: AuditContext) -> list[Finding]:
    code_vars = _collect_code_env_vars(ctx.source_files)
    sdk_vars = _collect_sdk_env_vars(ctx.sdk_source_files)
    # Merge: SDK reads count as "code reads" for the doc->code check, so docs
    # can mention SDK-side env vars without false alarms.
    for var, sites in sdk_vars.items():
        code_vars.setdefault(var, []).extend(sites)
    doc_vars = _collect_doc_env_vars(ctx)

    findings: list[Finding] = []

    for var, sites in code_vars.items():
        if var in _OS_ALLOWLIST or var in _TEST_ONLY_ALLOWLIST:
            continue
        if all(_site_is_test_only(p) for p in sites):
            continue
        if var.startswith("RCR_"):
            canonical = "SOLVELA_" + var[len("RCR_"):]
            if canonical in doc_vars or canonical in code_vars:
                continue
        if var not in doc_vars:
            first_site = next(
                (p for p in sites if not _site_is_test_only(p)),
                sites[0],
            )
            findings.append(
                Finding(
                    check=CHECK_NAME,
                    severity=Severity.WARNING,
                    message=f"`{var}` is read by code but never documented",
                    file=ctx.relpath(first_site),
                    suggestion="add it to .env.example and the relevant SDK/CLI doc page",
                )
            )

    for var, sites in doc_vars.items():
        if var in _OS_ALLOWLIST or var in _DOC_ONLY_ALLOWLIST:
            continue
        counterparts = {var}
        if var.startswith("SOLVELA_"):
            counterparts.add("RCR_" + var[len("SOLVELA_"):])
        elif var.startswith("RCR_"):
            counterparts.add("SOLVELA_" + var[len("RCR_"):])
        if any(c in code_vars for c in counterparts):
            continue
        first_doc = sites[0]
        findings.append(
            Finding(
                check=CHECK_NAME,
                severity=Severity.ERROR,
                message=(
                    f"`{var}` is documented but no code reads it; users following "
                    f"the docs will set a var that has no effect"
                ),
                file=ctx.relpath(first_doc[0]),
                line=first_doc[1],
                suggestion=(
                    "remove from the docs, OR fix the code to actually read it, "
                    "OR add to _DOC_ONLY_ALLOWLIST with a justification"
                ),
            )
        )

    return findings


def _collect_code_env_vars(source_files: list[Path]) -> dict[str, list[Path]]:
    out: dict[str, list[Path]] = {}
    for src in source_files:
        try:
            text = src.read_text(encoding="utf-8")
        except (OSError, UnicodeDecodeError):
            continue
        for m in _ENV_VAR_RUST_RE.finditer(text):
            # Capture from any of the named groups. env_with_fallback / read_env_var
            # take two positional names (canonical + legacy alias) — record both.
            for g in m.groups():
                if g:
                    out.setdefault(g, []).append(src)
    return out


def _collect_sdk_env_vars(sdk_files: list[Path]) -> dict[str, list[Path]]:
    out: dict[str, list[Path]] = {}
    for src in sdk_files:
        pattern = _ENV_VAR_SDK_RE.get(src.suffix)
        if pattern is None:
            continue
        try:
            text = src.read_text(encoding="utf-8")
        except (OSError, UnicodeDecodeError):
            continue
        for m in pattern.finditer(text):
            name = next((g for g in m.groups() if g), None)
            if name:
                out.setdefault(name, []).append(src)
    return out


def _collect_doc_env_vars(ctx: AuditContext) -> dict[str, list[tuple[Path, int]]]:
    out: dict[str, list[tuple[Path, int]]] = {}

    for doc in ctx.doc_files:
        for block in extract_code_blocks(doc, langs={"bash", "sh", "shell", "env", ""}):
            for lineno, line in block.lines():
                for m in _ENV_VAR_DOC_RE.finditer(line):
                    name = m.group(1) or m.group(2)
                    if not name or len(name) < 4:
                        continue
                    out.setdefault(name, []).append((doc, lineno))

    env_example = ctx.repo_root / ".env.example"
    if env_example.is_file():
        for i, line in enumerate(
            env_example.read_text(encoding="utf-8").splitlines(), start=1
        ):
            stripped = line.strip()
            if stripped.startswith("#") or not stripped:
                continue
            m = re.match(r"^([A-Z][A-Z0-9_]+)=", stripped)
            if m:
                out.setdefault(m.group(1), []).append((env_example, i))

    return out


def _site_is_test_only(path: Path) -> bool:
    parts = set(path.parts)
    if "tests" in parts or "benches" in parts:
        return True
    if path.name.endswith(("_test.rs", "_tests.rs")):
        return True
    return False
