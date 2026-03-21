---
paths:
  - "**/*.rs"
  - "**/Cargo.toml"
---
# Rust Security

> This file extends [common/security.md](../common/security.md) with Rust specific content.

## Secret Management

```rust
let api_key = std::env::var("API_KEY")
    .expect("API_KEY must be set");
```

- Secrets come from env vars only, **never** config files or hardcoded
- Implement custom `Debug` to redact secrets in logs

```rust
impl fmt::Debug for Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Config")
            .field("api_key", &"[REDACTED]")
            .finish()
    }
}
```

## Unsafe Code

- `unsafe` blocks require a `// SAFETY:` comment explaining the invariant
- Prefer safe abstractions; use `unsafe` only when necessary for FFI or performance
- Enable `unsafe_code = "warn"` in clippy lints

## Dependency Auditing

```bash
cargo audit              # Check for known vulnerabilities
cargo deny check         # Policy-based dependency checking
```

## Input Validation

- Validate all external input at system boundaries (API handlers, CLI args)
- Use newtype patterns to enforce validated data at the type level
