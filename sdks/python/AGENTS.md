<!-- Parent: ../AGENTS.md -->
<!-- Generated: 2026-04-16 | Updated: 2026-04-16 -->

# python

## Purpose
Python SDK for Solvela. Ships two import names during the RustyClawRouter → Solvela rebrand: `solvela` (canonical, keep current) and `rustyclawrouter` (legacy compatibility shim — avoid adding new code there).

## Key Files
| File | Description |
|------|-------------|
| `README.md` | Installation + quickstart |
| `pyproject.toml` | Packaging manifest (PEP 621) — dependencies, entry points |

## Subdirectories
| Directory | Purpose |
|-----------|---------|
| `solvela/` | Canonical package — Client, Wallet, x402, config, types (see `solvela/AGENTS.md`) |
| `rustyclawrouter/` | Legacy compat shim — re-exports `solvela` under the old name (see `rustyclawrouter/AGENTS.md`) |
| `tests/` | pytest suite (see `tests/AGENTS.md`) |
| `.pytest_cache/` | pytest cache (not checked in) |

## For AI Agents

### Working In This Directory
- Add all new code to `solvela/`. `rustyclawrouter/` is compatibility-only — shallow re-exports.
- Keep dependencies minimal — `httpx` or `requests` for HTTP, `solana`/`solders` for signing, `pydantic` optional.
- Support Python 3.10+ (check `pyproject.toml` for the pinned minimum).
- Never accept private keys as a file path parameter in the public API — take bytes or a base58 string explicitly.

### Testing Requirements
```bash
cd sdks/python && pytest
cd sdks/python && pytest tests/test_client.py -v
```

### Common Patterns
- PEP 8; type hints everywhere; `from __future__ import annotations`.
- Typed exceptions (`PaymentRequiredError`, `SigningError`, …) not generic `Exception`.

## Dependencies

### Internal
- Solvela gateway HTTP contract.

### External
- See `pyproject.toml`.

<!-- MANUAL: -->
