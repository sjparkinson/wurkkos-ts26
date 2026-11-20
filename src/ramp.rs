pub const LEVELS: u8 = 150;

pub const CEILING: u8 = LEVELS;

pub const PWM_TOP: u16 = 255;

pub const DSM_BITS: u8 = 7;

pub const DSM_FLOOR: u16 = 20;

pub const DSM_TOP: u16 = PWM_TOP << DSM_BITS;

const _: () = assert!(CEILING > 0 && CEILING <= LEVELS);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Output {
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

pub const FRACTION: u16 = 16;

const _: () = assert!(FRACTION.is_power_of_two());

pub fn faded(position: u16) -> Output {
    let position = position.min(u16::from(CEILING) * FRACTION);
    let Some(index) = (position / FRACTION).checked_sub(1) else {
        return Output::OFF;
    };
    let Some(&below) = TABLE.get(usize::from(index)) else {
        return Output::OFF;
    };
    let above = TABLE.get(usize::from(index.saturating_add(1)));
    let gap = above.map_or(0, |&above| {
        above
            .saturating_sub(below)
            .wrapping_mul(position % FRACTION)
            / FRACTION
    });
    Output {
        enable: true,
        dsm: below.saturating_add(gap),
    }
}

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
        assert_eq!(output(u8::MAX), output(CEILING));
    }

    #[test]
    fn the_ceiling_is_the_top_of_the_table() {
        assert_eq!(output(CEILING).dsm, DSM_TOP);
    }

    #[test]
    fn a_whole_level_fades_to_its_own_output() {
        for level in 0..=LEVELS {
            assert_eq!(faded(u16::from(level) * FRACTION), output(level));
        }
    }

    #[test]
    fn a_fade_climbs_between_levels_and_never_past_the_ceiling() {
        let top = u16::from(CEILING) * FRACTION;
        let mut last = output(1);
        for position in FRACTION + 1..=top + 2 * FRACTION {
            let now = faded(position);
            assert!(now.enable, "{position}");
            assert!(now.dsm >= last.dsm && now.dsm <= DSM_TOP, "{position}");
            last = now;
        }
        assert_eq!(faded(top + 1), output(CEILING));
        assert_eq!(faded(u16::MAX), output(CEILING));
    }

    #[test]
    fn no_gap_between_two_levels_wraps_a_faded_multiply() {
        let widest = TABLE.windows(2).map(|pair| pair[1] - pair[0]).max();
        assert!(widest.is_some_and(|gap| gap < 700), "{widest:?}");
    }

    #[test]
    fn a_fade_under_level_one_stays_off() {
        assert!((0..FRACTION).all(|position| faded(position) == Output::OFF));
    }

    #[test]
    fn halfway_between_two_levels_is_halfway_between_their_outputs() {
        let (below, above) = (output(140).dsm, output(141).dsm);
        let halfway = faded(140 * FRACTION + FRACTION / 2);
        assert_eq!(halfway.dsm, (below + above) / 2);
    }
}
