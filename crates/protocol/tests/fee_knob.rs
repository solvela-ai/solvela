//! Runtime platform-fee knob (`platform_fee_percent` / `set_platform_fee_percent`).
//!
//! The knob is PROCESS-GLOBAL, so this lives in its own test binary (own
//! process) and runs the whole sequence in ONE `#[test]` — parallel tests
//! sharing the static would race. Never move setter tests into the lib test
//! module: `serializes_nested_wire_shape` (model.rs) reads the live value and
//! would flake against a concurrent `set(0)`.

use solvela_protocol::{platform_fee_percent, set_platform_fee_percent, PLATFORM_FEE_PERCENT};

#[test]
fn fee_knob_defaults_to_constant_and_rejects_out_of_range() {
    // Default: the compile-time constant (5%) until the gateway sets it at boot.
    assert_eq!(PLATFORM_FEE_PERCENT, 5);
    assert_eq!(
        platform_fee_percent(),
        PLATFORM_FEE_PERCENT,
        "live fee must default to the constant before any set"
    );

    // 0% is a legal, production-exercised wire shape (vendor path emits fee_percent 0).
    set_platform_fee_percent(0).expect("0% must be accepted");
    assert_eq!(platform_fee_percent(), 0);

    // >100% is rejected, never clamped, and the live value is untouched.
    let err = set_platform_fee_percent(101).expect_err("101% must be rejected");
    assert!(
        err.to_string().contains("101"),
        "error must name the rejected value: {err}"
    );
    assert_eq!(
        platform_fee_percent(),
        0,
        "a rejected set must leave the live value unchanged (no clamp, no partial apply)"
    );
    assert!(
        set_platform_fee_percent(u8::MAX).is_err(),
        "u8::MAX must be rejected"
    );
    assert_eq!(platform_fee_percent(), 0);

    // 100% is the inclusive upper bound.
    set_platform_fee_percent(100).expect("100% must be accepted");
    assert_eq!(platform_fee_percent(), 100);

    // Back to the default.
    set_platform_fee_percent(5).expect("5% must be accepted");
    assert_eq!(platform_fee_percent(), 5);
}
