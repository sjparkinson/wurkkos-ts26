//! T2 in docs/breakout.md, a number out. Throwaway, and never in the real
//! firmware.
//!
//! Blinks a two-digit number on the main LED, tens on long blinks and units on
//! short ones, with a quick blip for a zero. A dark pause comes before the
//! tens, between the digits and after the units.

/// How long each part lasts, in 64 Hz ticks.
pub const LONG: u8 = 32;
pub const SHORT: u8 = 8;
pub const BLIP: u8 = 2;
/// Dark between two blinks of the same digit.
pub const GAP: u8 = 24;
/// Dark before the tens, between the digits and after the units.
pub const PAUSE: u8 = 96;

/// Tenths of a volt from centivolts, rounded, as (volts, tenths), each
/// capped at 9.
///
/// `/ 10` would need `__mulsi3`, and so does a subtraction loop, which LLVM
/// turns back into one, so each digit is four fixed steps of 8, 4, 2 and 1 of
/// its unit.
pub fn decivolts(centivolts: u16) -> (u8, u8) {
    let left = centivolts.saturating_add(5);
    if left >= 1000 {
        return (9, 9);
    }
    let (volts, left) = digit(left, 100);
    let (tenths, _) = digit(left, 10);
    (volts, tenths)
}

/// How many `unit`s fit in `value`, for a count up to 15, and what's left.
fn digit(mut value: u16, unit: u16) -> (u8, u16) {
    let mut count = 0u8;
    let mut bit = 8u8;
    while bit > 0 {
        let step = unit.saturating_mul(u16::from(bit));
        if value >= step {
            value = value.wrapping_sub(step);
            count |= bit;
        }
        bit >>= 1;
    }
    (count, value)
}

#[derive(Clone, Copy, Debug)]
pub struct Blink {
    tens: u8,
    units: u8,
    /// 0 for the pause before the tens, 1 the tens, 2 the units.
    stage: u8,
    /// Blinks still to start in this digit.
    left: u8,
    lit: bool,
    /// Ticks left in this blink or dark spell, counting this one.
    ticks: u8,
}

impl Blink {
    pub const fn new(tens: u8, units: u8) -> Self {
        Self {
            tens,
            units,
            stage: 0,
            left: 0,
            lit: false,
            // One more, since the first tick counts down before it returns.
            ticks: PAUSE.saturating_add(1),
        }
    }

    /// Whether the LED's lit on this tick.
    pub const fn lit(&self) -> bool {
        self.lit
    }

    /// One tick. `Some(true)` for lit, `Some(false)` for dark, `None` once
    /// it's finished.
    pub fn tick(&mut self) -> Option<bool> {
        if self.ticks > 1 {
            self.ticks = self.ticks.wrapping_sub(1);
            return Some(self.lit);
        }
        if self.lit {
            self.lit = false;
            self.ticks = if self.left > 0 { GAP } else { PAUSE };
            return Some(false);
        }
        if self.left == 0 {
            self.stage = self.stage.wrapping_add(1);
            let digit = match self.stage {
                1 => self.tens,
                2 => self.units,
                _ => return None,
            };
            self.left = digit.max(1);
        }
        self.left = self.left.wrapping_sub(1);
        self.lit = true;
        let (digit, length) = if self.stage == 1 {
            (self.tens, LONG)
        } else {
            (self.units, SHORT)
        };
        self.ticks = if digit == 0 { BLIP } else { length };
        Some(true)
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use std::vec::Vec;

    /// The run of lit and dark spells as (lit, ticks).
    fn spells(tens: u8, units: u8) -> Vec<(bool, u8)> {
        let mut blink = Blink::new(tens, units);
        let mut out: Vec<(bool, u8)> = Vec::new();
        while let Some(lit) = blink.tick() {
            match out.last_mut() {
                Some((was, n)) if *was == lit => *n += 1,
                _ => out.push((lit, 1)),
            }
        }
        out
    }

    #[test]
    fn three_point_six() {
        let mut expected = std::vec![(false, PAUSE)];
        for i in 0..3 {
            expected.push((true, LONG));
            expected.push((false, if i < 2 { GAP } else { PAUSE }));
        }
        for i in 0..6 {
            expected.push((true, SHORT));
            expected.push((false, if i < 5 { GAP } else { PAUSE }));
        }
        assert_eq!(spells(3, 6), expected);
    }

    #[test]
    fn zeros_are_blips() {
        assert_eq!(
            spells(4, 0),
            std::vec![
                (false, PAUSE),
                (true, LONG),
                (false, GAP),
                (true, LONG),
                (false, GAP),
                (true, LONG),
                (false, GAP),
                (true, LONG),
                (false, PAUSE),
                (true, BLIP),
                (false, PAUSE),
            ]
        );
        assert_eq!(
            spells(0, 0),
            std::vec![
                (false, PAUSE),
                (true, BLIP),
                (false, PAUSE),
                (true, BLIP),
                (false, PAUSE),
            ]
        );
    }

    #[test]
    fn stays_finished() {
        let mut blink = Blink::new(1, 1);
        while blink.tick().is_some() {}
        assert_eq!(blink.tick(), None);
    }

    #[test]
    fn rounds_to_tenths() {
        assert_eq!(decivolts(360), (3, 6));
        assert_eq!(decivolts(364), (3, 6));
        assert_eq!(decivolts(365), (3, 7));
        assert_eq!(decivolts(395), (4, 0));
        assert_eq!(decivolts(0), (0, 0));
        assert_eq!(decivolts(u16::MAX), (9, 9));
    }
}
