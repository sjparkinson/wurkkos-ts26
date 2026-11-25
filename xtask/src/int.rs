//! Integer conversions for const items, where `From` and `TryFrom` can't be
//! called.

#[expect(
    clippy::as_conversions,
    reason = "widening in a const fn, where From can't be called"
)]
pub const fn u64_from_usize(value: usize) -> u64 {
    value as u64
}

#[expect(
    clippy::as_conversions,
    clippy::cast_possible_truncation,
    reason = "checked by the assert, in a const fn, where TryFrom can't be called"
)]
pub const fn u8_of(value: usize) -> u8 {
    assert!(value <= 0xFF);
    value as u8
}

#[expect(
    clippy::as_conversions,
    clippy::cast_possible_truncation,
    reason = "checked by the assert, in a const fn, where TryFrom can't be called"
)]
pub const fn u16_of(value: usize) -> u16 {
    assert!(value <= 0xFFFF);
    value as u16
}

#[expect(
    clippy::as_conversions,
    clippy::cast_possible_truncation,
    reason = "checked by the assert, in a const fn, where TryFrom can't be called"
)]
pub const fn u32_of(value: usize) -> u32 {
    assert!(value <= 0xFFFF_FFFF);
    value as u32
}

#[expect(
    clippy::as_conversions,
    reason = "widening in a const fn, where From can't be called"
)]
pub const fn u32_from_u16(value: u16) -> u32 {
    value as u32
}
