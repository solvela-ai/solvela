"""Journey scenario for the Rust CLI quickstart.

Mirrors `dashboard/content/docs/sdks/rust.mdx`. If the doc tells the user
to `export FOO=BAR`, this scenario declares that env var. If the doc shows
`solvela chat "..."`, this scenario runs that command. If the doc DOESN'T
mention an env var that the CLI actually requires, this scenario WILL fail
at the chat step — which is exactly the bug class the journey check catches.

When updating the quickstart doc, update this scenario in the same PR.
"""

from __future__ import annotations

from journey.scenario import Scenario, Step


# Env vars the Rust CLI quickstart (rust.mdx lines 25-27) tells the user
# to export. If a future doc edit removes one of these and the CLI still
# needs it, the chat smoke test below will fail.
_DOC_ENV: dict[str, str] = {
    "SOLVELA_API_URL": "https://api.solvela.ai",
    "SOLANA_RPC_URL": "https://api.mainnet-beta.solana.com",
    "RUST_LOG": "info",
}


_HELP_STEPS: tuple[Step, ...] = (
    Step(
        name="version",
        cmd=["solvela", "--version"],
        expected_exit=0,
        must_contain=("solvela",),
        why="The binary should at least report its version; this catches "
        "a totally-broken install.",
    ),
    Step(
        name="top-level-help",
        cmd=["solvela", "--help"],
        expected_exit=0,
        must_contain=(
            "Commands:",
            "wallet",
            "chat",
            "models",
            "health",
        ),
        why="Top-level --help should show all subcommands a quickstart user "
        "is told to run.",
    ),
    Step(
        name="chat-help",
        cmd=["solvela", "chat", "--help"],
        expected_exit=0,
        must_contain=("PROMPT",),
        why="`chat --help` should advertise the positional PROMPT argument.",
    ),
)


_WALLET_STEPS: tuple[Step, ...] = (
    Step(
        name="wallet-init",
        cmd=["solvela", "wallet", "init"],
        expected_exit=0,
        must_contain=("address",),
        why="`wallet init` is step 1 of any paid command; it must succeed "
        "with a clear next-action message.",
    ),
)


_CHAT_STEPS: tuple[Step, ...] = (
    Step(
        name="chat-smoke-test",
        cmd=["solvela", "chat", "test", "--yes"],
        expected_exit=None,
        must_not_contain=("SOLANA_RPC_URL required",),
        timeout_seconds=180,
        why="The original bug class: if the doc's env exports don't "
        "include SOLANA_RPC_URL, the CLI errors with 'SOLANA_RPC_URL "
        "required' before reaching payment. Any other failure (insufficient "
        "funds, no ATA, network) is acceptable — those happen because we "
        "can't fund a wallet in CI, not because the docs are wrong.",
    ),
)


SCENARIO: Scenario = Scenario(
    name="rust_cli",
    doc_relpath="dashboard/content/docs/sdks/rust.mdx",
    description=(
        "Follow the Rust CLI quickstart verbatim and catch experiential "
        "drift (the SOLANA_RPC_URL bug class)."
    ),
    env=_DOC_ENV,
    steps=_HELP_STEPS + _WALLET_STEPS + _CHAT_STEPS,
    cli_dependent=True,
)
