//! Everything that decides what the torch does, with no hardware in sight.
//!
//! This crate is `no_std` so the same code runs on the ATtiny1616 and in
//! `cargo test` on your laptop. The firmware binary (`src/main.rs`) is a thin
//! loop that feeds switch samples in and writes the output back.

#![no_std]
// The library is the safety logic, so nothing in it may panic, index out of
// range, overflow or truncate without saying why it can't. Tests may.
// This list, the one on the `testing` module below and the one in
// src/main.rs must match. The xtask test `lints::restriction_lint_lists_match`
// keeps them in step.
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
// Fakes and helpers for tests and xtask's simulators, never in the firmware,
// so the restrictions above don't apply. The list must match the one above,
// which the xtask test `lints::restriction_lint_lists_match` checks.
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

/// How often the firmware samples the switch and steps the UI. The RTC's
/// periodic interrupt timer divides its 32.768 kHz clock by 512.
pub const TICKS_PER_SECOND: u16 = 64;
