//! The 150 levels, and what each one asks of the boost converter.
//!
//! Levels are in delta-sigma units, the same 15-bit scale Anduril uses on this
//! board: 0 to 32640, which is a PWM duty of 0 to 255 with seven more bits
//! below it. docs/hardware.md has why. The curve here is my own cubic, not
//! Anduril's table, and nobody's measured what it comes out as in lumens.

/// How many levels the ramp has. Level 0 is off.
pub const LEVELS: u8 = 150;

/// The highest level the firmware will run at, until thermal regulation has
/// been checked on a real light. Turbo pulls about 11 A from the cell, and the
/// thermal constants have never met this torch. Raise it to [`LEVELS`] then.
pub const CEILING: u8 = 120;

/// TCA0's period. Dual-slope PWM at 10 MHz with a TOP of 255 runs at
/// 10 MHz / (2 × 255) ≈ 19.6 kHz, well above anything you'd hear or see.
pub const PWM_TOP: u16 = 255;

/// How many delta-sigma bits sit below the PWM duty.
pub const DSM_BITS: u8 = 7;

/// Level 1. ToyKeeper's sample wouldn't light steadily below this
/// (docs/hardware.md).
pub const DSM_FLOOR: u16 = 20;

/// Level 150, which is a duty of 255 with the delta-sigma bits clear.
pub const DSM_TOP: u16 = PWM_TOP << DSM_BITS;

const _: () = assert!(CEILING > 0 && CEILING <= LEVELS);

/// What the main LED's driver should be doing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Output {
    /// The boost converter's enable pin (PB2).
    pub enable: bool,
    /// The brightness on PB0, from 0 to [`DSM_TOP`].
    pub dsm: u16,
}

impl Output {
    pub const OFF: Self = Self {
        enable: false,
        dsm: 0,
    };
}

static TABLE: [u16; LEVELS as usize] = table();

/// The output for `level`. Level 0 is off, and anything over [`CEILING`] runs
/// at the ceiling.
pub fn output(level: u8) -> Output {
    if level == 0 {
        return Output::OFF;
    }
    let level = if level > CEILING { CEILING } else { level };
    Output {
        enable: true,
        dsm: TABLE[usize::from(level) - 1],
    }
}

/// Worked out at compile time, so the wide maths never reaches the chip. A
/// cubic this flat at the bottom would repeat itself there, so each level is
/// held at least one step over the last.
const fn table() -> [u16; LEVELS as usize] {
    let mut table = [0; LEVELS as usize];
    let span = (DSM_TOP - DSM_FLOOR) as u64;
    let last = LEVELS as u64 - 1;
    let mut i = 0;
    while i < LEVELS as usize {
        let step = i as u64;
        let curve = DSM_FLOOR + (span * step * step * step / (last * last * last)) as u16;
        table[i] = if i > 0 && curve <= table[i - 1] {
            table[i - 1] + 1
        } else {
            curve
        };
        i += 1;
    }
    table
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn level_zero_is_off() {
        assert_eq!(output(0), Output::OFF);
    }

    #[test]
    fn the_table_runs_from_the_floor_to_the_top() {
        assert_eq!(TABLE[0], DSM_FLOOR);
        assert_eq!(TABLE[LEVELS as usize - 1], DSM_TOP);
    }

    #[test]
    fn every_level_is_brighter_than_the_last() {
        assert!(TABLE.windows(2).all(|pair| pair[0] < pair[1]));
    }

    #[test]
    fn every_level_turns_the_boost_converter_on() {
        assert!((1..=LEVELS).all(|level| output(level).enable));
    }

    #[test]
    fn nothing_runs_above_the_ceiling() {
        assert_eq!(output(LEVELS), output(CEILING));
        assert_eq!(output(u8::MAX), output(CEILING));
        assert!(output(CEILING).dsm < DSM_TOP);
    }
}
