//! The MCU's temperature sensor, from four accumulated conversions and the
//! `SIGROW.TEMPSENSE1`/`TEMPSENSE0` calibration (DS40002204A §30.3.2.6).
//!
//! `d * gain` runs to 21 bits and there's no runtime 32-bit multiply, so
//! [`sixteenths`] splits `d` into bytes. The split is exact, since
//! `hi * g * 256` has nothing in its low six bits.

use crate::{u16_from, u32_from};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Calibration {
    pub offset: i8,
    pub gain: u8,
}

impl Calibration {
    pub fn sixteenths(self, sum: u16) -> u16 {
        sixteenths(sum, self.offset, self.gain)
    }
}

/// Keeps every step of [`sixteenths`] inside a `u16`, at over three times
/// what the ADC can produce.
const MAX_DIFFERENCE: u16 = 0x3FFF;

const _: () = {
    let (hi, lo, g) = (
        u32_from(MAX_DIFFERENCE >> 8),
        0xFF,
        u32_from(u16_from(u8::MAX)),
    );
    assert!(lo * g + 0x20 <= u32_from(u16::MAX));
    assert!(hi * g * 4 + ((lo * g + 0x20) >> 6) <= u32_from(u16::MAX));
};

#[expect(
    clippy::arithmetic_side_effects,
    reason = "an i8 times -4 is within -508..=512, and the assert above bounds the rest"
)]
pub fn sixteenths(sum: u16, offset: i8, gain: u8) -> u16 {
    let d = sum
        .saturating_add_signed(-4 * i16::from(offset))
        .min(MAX_DIFFERENCE);
    let (hi, lo, g) = (d >> 8, d & 0xFF, u16::from(gain));
    hi * g * 4 + ((lo * g + 0x20) >> 6)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reference(sum: u16, offset: i8, gain: u8) -> u16 {
        let d = (i32::from(sum) - 4 * i32::from(offset)).clamp(0, i32::from(MAX_DIFFERENCE));
        ((d * i32::from(gain) + 0x20) >> 6) as u16
    }

    #[test]
    fn matches_the_sum_for_every_accumulated_reading_and_calibration() {
        for sum in 0..=4 * 1023 {
            for offset in i8::MIN..=i8::MAX {
                for gain in (u8::MIN..=u8::MAX).step_by(5) {
                    assert_eq!(
                        sixteenths(sum, offset, gain),
                        reference(sum, offset, gain),
                        "sum {sum}, offset {offset}, gain {gain}",
                    );
                }
            }
        }
    }

    #[test]
    fn the_split_is_exact_for_every_16_bit_sum() {
        for sum in u16::MIN..=u16::MAX {
            for gain in u8::MIN..=u8::MAX {
                assert_eq!(
                    sixteenths(sum, 0, gain),
                    reference(sum, 0, gain),
                    "sum {sum}, gain {gain}"
                );
            }
        }
    }

    #[test]
    fn four_conversions_land_within_half_a_kelvin_of_the_datasheet() {
        for reading in 0..1024_i32 {
            for offset in i8::MIN..=i8::MAX {
                for gain in u8::MIN..=u8::MAX {
                    let d = (reading - i32::from(offset)).max(0);
                    let datasheet = (d * i32::from(gain) + 0x80) >> 8;
                    let ours = i32::from(sixteenths(4 * reading as u16, offset, gain));
                    assert!(
                        (ours - 16 * datasheet).abs() <= 8,
                        "reading {reading}, offset {offset}, gain {gain}",
                    );
                }
            }
        }
    }

    #[test]
    fn the_calibration_does_the_same_sums() {
        let calibration = Calibration {
            offset: -10,
            gain: 141,
        };
        assert_eq!(calibration.sixteenths(2124), sixteenths(2124, -10, 141));
        assert_eq!(calibration.sixteenths(2124), 298 * 16);
    }

    #[test]
    fn one_count_of_the_sum_is_a_fraction_of_a_kelvin() {
        let at = |sum| sixteenths(sum, -10, 141);
        assert_eq!(at(2125) - at(2124), 2);
        assert_eq!(at(2128) - at(2124), 8);
    }

    #[test]
    fn a_sum_below_the_offset_is_zero() {
        assert_eq!(sixteenths(10, 20, 200), 0);
    }

    #[test]
    fn a_sum_nothing_could_produce_stays_far_from_plausible() {
        assert!(sixteenths(u16::MAX, -128, 141) > 423 * 16);
    }
}
