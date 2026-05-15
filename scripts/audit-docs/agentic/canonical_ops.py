"""Canonical-operation registry for SDK parity sweeps.

Each canonical operation is something every payment client must implement:
init wallet, send a paid chat, list available models. For each, this module
declares which SDK source files implement it and which reference files
(Rust CLI + protocol wire types) define the authoritative behavior.

The agent gets all of them side by side and is asked to find cases where:
  - one SDK's wire format diverges from the protocol (the @0.2.1 bug class)
  - SDKs name the same operation differently in ways readers won't expect
  - one SDK does validation / retry / error-handling the others don't
  - one SDK is missing the operation entirely

Add a new operation by appending to `CANONICAL_OPS`. Add a new SDK by adding
it to each `OpSpec.sdk_files` mapping.
"""

from __future__ import annotations

from dataclasses import dataclass
from pathlib import Path


@dataclass(frozen=True)
class OpSpec:
    name: str
    description: str
    sdk_files: dict[str, tuple[str, ...]]
    """SDK name -> tuple of source files for this op (relpath from repo root)."""
    reference_files: tuple[str, ...] = ()
    """Authoritative source-of-truth files (protocol wire types, Rust CLI).
    The agent treats these as ground truth."""
    readme_anchors: dict[str, tuple[tuple[str, str], ...]] = None  # type: ignore[assignment]
    """SDK name -> tuple of (readme_relpath, heading_substring) pairs."""

    def __post_init__(self) -> None:
        if self.readme_anchors is None:
            object.__setattr__(self, "readme_anchors", {})


_PROTOCOL_TYPES = (
    "crates/protocol/src/payment.rs",
    "crates/protocol/src/model.rs",
    "crates/protocol/src/chat.rs",
)
_CLI_CHAT = ("crates/cli/src/commands/chat.rs",)
_CLI_WALLET = ("crates/cli/src/commands/wallet.rs",)


CANONICAL_OPS: tuple[OpSpec, ...] = (
    OpSpec(
        name="init_wallet",
        description=(
            "Generate a Solana keypair, persist it to disk under a "
            "well-known path, and print the public address so the user "
            "can fund it."
        ),
        sdk_files={
            "typescript": (
                "sdks/typescript/src/wallet.ts",
                "sdks/typescript/src/signer.ts",
            ),
            "python": (
                "sdks/python/src/solvela/wallet.py",
                "sdks/python/src/solvela/signer.py",
            ),
            "go": ("sdks/go/wallet.go", "sdks/go/signer.go"),
        },
        reference_files=_CLI_WALLET,
        readme_anchors={
            "typescript": (("sdks/typescript/README.md", "wallet"),),
            "python": (("sdks/python/README.md", "wallet"),),
            "go": (("sdks/go/README.md", "wallet"),),
        },
    ),
    OpSpec(
        name="send_paid_chat",
        description=(
            "Send a chat completion request, handle the 402 Payment Required "
            "response, sign a USDC-SPL transfer (or escrow deposit), retry "
            "with the PAYMENT-SIGNATURE header. The full x402 handshake."
        ),
        sdk_files={
            "typescript": (
                "sdks/typescript/src/client.ts",
                "sdks/typescript/src/transport.ts",
            ),
            "python": (
                "sdks/python/src/solvela/client.py",
                "sdks/python/src/solvela/transport.py",
            ),
            "go": ("sdks/go/client.go", "sdks/go/transport.go"),
        },
        reference_files=_PROTOCOL_TYPES + _CLI_CHAT,
        readme_anchors={
            "typescript": (("sdks/typescript/README.md", "chat"),),
            "python": (("sdks/python/README.md", "chat"),),
            "go": (("sdks/go/README.md", "chat"),),
        },
    ),
    OpSpec(
        name="list_models",
        description=(
            "GET /v1/models and surface the registry to the user: model id, "
            "provider, per-token pricing, capability flags."
        ),
        sdk_files={
            "typescript": (
                "sdks/typescript/src/client.ts",
                "sdks/typescript/src/types.ts",
            ),
            "python": (
                "sdks/python/src/solvela/client.py",
                "sdks/python/src/solvela/types.py",
            ),
            "go": ("sdks/go/client.go", "sdks/go/types.go"),
        },
        reference_files=(
            "crates/protocol/src/model.rs",
            "crates/cli/src/commands/models.rs",
        ),
        readme_anchors={
            "typescript": (("sdks/typescript/README.md", "models"),),
            "python": (("sdks/python/README.md", "models"),),
            "go": (("sdks/go/README.md", "models"),),
        },
    ),
)


@dataclass(frozen=True)
class OpContext:
    """Bundle of source-of-truth excerpts + per-SDK implementations for one
    canonical operation, ready to be inlined into the agent prompt."""

    op: OpSpec
    sdk_sources: dict[str, list[tuple[str, str]]]
    """SDK name -> list of (relpath, content) pairs."""
    reference_sources: list[tuple[str, str]]
    """List of (relpath, content) pairs from reference_files."""
    readme_excerpts: dict[str, list[tuple[str, str, int]]]
    """SDK name -> list of (relpath, excerpt_text, start_line)."""


def gather_op_context(repo_root: Path, op: OpSpec) -> OpContext:
    """Read every file declared in `op` and return the bundle. Missing files
    are silently skipped so the audit still works in worktrees missing one
    SDK."""
    sdk_sources: dict[str, list[tuple[str, str]]] = {}
    for sdk_name, files in op.sdk_files.items():
        bucket: list[tuple[str, str]] = []
        for relpath in files:
            content = _read(repo_root / relpath)
            if content is not None:
                bucket.append((relpath, content))
        if bucket:
            sdk_sources[sdk_name] = bucket

    reference_sources: list[tuple[str, str]] = []
    for relpath in op.reference_files:
        content = _read(repo_root / relpath)
        if content is not None:
            reference_sources.append((relpath, content))

    readme_excerpts: dict[str, list[tuple[str, str, int]]] = {}
    for sdk_name, anchors in op.readme_anchors.items():
        bucket2: list[tuple[str, str, int]] = []
        for readme_relpath, heading_sub in anchors:
            excerpt = _extract_section(repo_root / readme_relpath, heading_sub)
            if excerpt is not None:
                bucket2.append((readme_relpath, excerpt[0], excerpt[1]))
        if bucket2:
            readme_excerpts[sdk_name] = bucket2

    return OpContext(
        op=op,
        sdk_sources=sdk_sources,
        reference_sources=reference_sources,
        readme_excerpts=readme_excerpts,
    )


def _read(path: Path) -> str | None:
    try:
        return path.read_text(encoding="utf-8")
    except (OSError, UnicodeDecodeError):
        return None


def _extract_section(path: Path, heading_sub: str) -> tuple[str, int] | None:
    """Find a markdown heading containing `heading_sub` (case-insensitive)
    and return everything up to the next heading at same-or-higher level.

    Returns (excerpt_text, start_line). start_line is 1-based.
    """
    if not path.is_file():
        return None
    try:
        text = path.read_text(encoding="utf-8")
    except (OSError, UnicodeDecodeError):
        return None

    target_lower = heading_sub.lower()
    lines = text.splitlines()
    start: int | None = None
    start_level: int = 0
    for i, line in enumerate(lines):
        stripped = line.lstrip("#")
        level = len(line) - len(stripped)
        if level == 0 or level > 6:
            continue
        heading_text = stripped.strip().lower()
        if target_lower in heading_text:
            start = i
            start_level = level
            break

    if start is None:
        return None

    end = len(lines)
    for j in range(start + 1, len(lines)):
        line = lines[j]
        stripped = line.lstrip("#")
        lvl = len(line) - len(stripped)
        if 0 < lvl <= start_level:
            end = j
            break

    return ("\n".join(lines[start:end]), start + 1)
