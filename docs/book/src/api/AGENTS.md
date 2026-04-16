<!-- Parent: ../AGENTS.md -->
<!-- Generated: 2026-04-16 | Updated: 2026-04-16 -->

# api

## Purpose
HTTP API reference — endpoint list, request/response shapes, headers, error codes, 402 semantics. Authoritative description of the gateway's public surface.

## For AI Agents

### Working In This Directory
- Keep in sync with `crates/gateway/src/routes/` — when an endpoint is added, updated, or removed, update the reference here too.
- Document both the 2xx success shape and the 402 shape (it's the most distinctive part of Solvela's API).
- Use real JSON examples; keep them minimal but valid.

### Testing Requirements
- Review by someone other than the author; no automated tests.

### Common Patterns
- One page per endpoint or per cohesive endpoint group.
- Document headers explicitly (`PAYMENT-SIGNATURE`, `x-request-id`, API-key header).

## Dependencies

### Internal
- `crates/gateway/src/routes/` — authoritative source.

### External
_(none)_

<!-- MANUAL: -->
