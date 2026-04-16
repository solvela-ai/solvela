<!-- Parent: ../AGENTS.md -->
<!-- Generated: 2026-04-16 | Updated: 2026-04-16 -->

# solvela

## Purpose
Canonical Python package for the Solvela SDK. One module per concern.

## Key Files
| File | Description |
|------|-------------|
| `__init__.py` | Public exports — `Client`, `Wallet`, `Config`, error types |
| `client.py` | `Client` — gateway HTTP wrapper; handles 402→sign→retry, streaming chat |
| `config.py` | Typed config (gateway URL, default model, timeout, wallet) + env loader |
| `wallet.py` | Solana keypair loading + signing helpers |
| `x402.py` | x402 header encoding + payment-required parsing |
| `types.py` | Wire-format dataclasses — chat request/response, payment required, cost breakdown |

## Subdirectories
_(none)_

## For AI Agents

### Working In This Directory
- Every public function/class has a type hint and a docstring.
- `__init__.py` is the public surface; everything consumers need flows through it.
- Never write private-key bytes to disk; never log them.
- Keep `client.py` dependency-light — prefer `httpx.AsyncClient` for async, `httpx.Client` for sync.

### Testing Requirements
```bash
pytest sdks/python/solvela        # (optional — tests live in ../tests)
pytest sdks/python                 # full suite
```

### Common Patterns
- Dataclasses or `pydantic` models for wire types (match what `types.py` does today).
- Explicit exception hierarchy rooted at `SolvelaError`.
- Context manager support where resources are held (`with Client(...) as c:`).

## Dependencies

### Internal
_(none — leaf package)_

### External
- `httpx`, `solana`/`solders`, `base58`, plus pinned crypto libs from `pyproject.toml`.

<!-- MANUAL: -->
