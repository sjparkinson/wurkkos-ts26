#![no_std]
// Must match the list on `testing` below and the one in src/main.rs.
#![cfg_attr(
    not(test),
    deny(
        clippy::indexing_slicing,
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::unreachable,
        clippy::arithmetic_side_effects,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_possible_wrap,
        clippy::as_conversions,
    )
)]

pub mod battery;
pub mod board;
pub mod button;
pub mod gradual;
pub mod limits;
pub mod ramp;
pub mod regs;
pub mod selftest;
pub mod sensors;
pub mod temp;
#[cfg(any(test, feature = "testing"))]
#[allow(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    clippy::arithmetic_side_effects,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_possible_wrap,
    clippy::as_conversions
)]
pub mod testing;
pub mod thermal;
pub mod torch;
pub mod ui;
pub mod watchdog;

/// 32.768 kHz over the PIT's 512.
pub const TICKS_PER_SECOND: u16 = 64;

#[expect(
    clippy::as_conversions,
    reason = "widening in a const fn, where From can't be called"
)]
pub(crate) const fn u16_from(value: u8) -> u16 {
    value as u16
}

#[expect(
    clippy::as_conversions,
    reason = "widening in a const fn, where From can't be called"
)]
pub(crate) const fn u32_from(value: u16) -> u32 {
    value as u32
}
