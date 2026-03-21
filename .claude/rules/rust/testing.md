---
paths:
  - "**/*.rs"
  - "**/Cargo.toml"
---
# Rust Testing

> This file extends [common/testing.md](../common/testing.md) with Rust specific content.

## Framework

Use `#[cfg(test)]` modules for unit tests, `tests/` directory for integration tests.

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_something() {
        // Arrange
        // Act
        // Assert
    }

    #[tokio::test]
    async fn test_async_something() {
        // Arrange
        // Act
        // Assert
    }
}
```

## Coverage

```bash
cargo test                    # All tests
cargo test -p crate_name      # Single crate
cargo test -- test_name        # Single test by name
cargo test -- --nocapture      # Show stdout/tracing output
```

## Assertions

- Use `assert_eq!`, `assert_ne!`, `assert!` with descriptive messages
- For complex assertions, consider `pretty_assertions` crate
- Never use `.unwrap()` in tests without a comment explaining why it's safe

## Async Tests

Always use `#[tokio::test]` for async test functions. Use `tokio::time::timeout` to prevent hanging tests.

## Reference

See skill: `rust-pro` for advanced Rust testing patterns.
