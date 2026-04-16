<!-- Parent: ../AGENTS.md -->
<!-- Generated: 2026-04-16 | Updated: 2026-04-16 -->

# tests

## Purpose
pytest suite for the Python SDK. Covers client behaviour, wallet signing, x402 header handling, typed config, and wire types.

## Key Files
| File | Description |
|------|-------------|
| `__init__.py` | Marks the directory as a package |
| `test_client.py` | Client HTTP behaviour — 200, 402→sign→retry, streaming, error mapping |
| `test_config.py` | Config parsing — env var resolution, defaults, validation |
| `test_types.py` | Wire-type round-tripping — serialization parity with the gateway |
| `test_wallet.py` | Wallet — keypair loading from bytes/base58, signing determinism |
| `test_x402.py` | x402 header encoding/decoding |
| `__pycache__/` | Generated (ignored) |

## Subdirectories
_(none)_

## For AI Agents

### Working In This Directory
- Mock HTTP with `httpx.MockTransport` or `respx` — no network calls.
- Use deterministic keypairs for signing tests (fixed 32-byte seed).
- Every test file corresponds to one module under `../solvela/`.
- Add regression tests for every bug fix, pinned with a comment referencing the issue.

### Testing Requirements
```bash
pytest sdks/python
pytest sdks/python -v
pytest sdks/python/tests/test_client.py::test_payment_required -v
```

### Common Patterns
- `@pytest.fixture` for shared client/wallet setup.
- Arrange / Act / Assert blocks.

## Dependencies

### Internal
- `../solvela/` — code under test.

### External
- `pytest`, `respx` or `httpx.MockTransport` (see `../pyproject.toml`).

<!-- MANUAL: -->
