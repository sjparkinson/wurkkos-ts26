//! The curve is unmeasured in lumens.

pub const LEVELS: u8 = 150;

/// Held below [`LEVELS`] until thermal regulation has been checked on a real
/// light, since turbo pulls about 11 A. Unconfirmed.
pub const CEILING: u8 = 120;

/// Dual-slope at 10 MHz, so about 19.6 kHz.
pub const PWM_TOP: u16 = 255;

pub const DSM_BITS: u8 = 7;

/// One sample wouldn't light steadily below this. Unconfirmed on this torch.
pub const DSM_FLOOR: u16 = 20;

pub const DSM_TOP: u16 = PWM_TOP << DSM_BITS;

const _: () = assert!(CEILING > 0 && CEILING <= LEVELS);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Output {
    /// PB2.
    pub enable: bool,
    pub dsm: u16,
}

impl Output {
    pub const OFF: Self = Self {
        enable: false,
        dsm: 0,
    };
}

#[expect(
    clippy::as_conversions,
    reason = "widening u8 to usize for an array length, where From can't be called"
)]
static TABLE: [u16; LEVELS as usize] = table();

pub fn output(level: u8) -> Output {
    let Some(index) = level.min(CEILING).checked_sub(1) else {
        return Output::OFF;
    };
    match TABLE.get(usize::from(index)) {
        Some(&dsm) => Output { enable: true, dsm },
        None => Output::OFF,
    }
}

/// `const fn`, since the u64 maths would need `__mulsi3` and up at runtime.
#[expect(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation,
    clippy::as_conversions,
    reason = "only ever run to build TABLE, where an overflow or a bad index fails the build, \
              the cubic term is at most span, which came from a u16, and From can't be called \
              in a const fn"
)]
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
