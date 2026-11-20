use crate::gradual::Ceiling;
use crate::ramp::CEILING;
use crate::sensors::{CYCLE_TICKS, FULL_SCALE};
use crate::{TICKS_PER_SECOND, u16_from, u32_from};

/// The stock firmware's calibration for this board, unmeasured on this torch.
pub const SLOPE: u16 = 1094;

/// 1.1 V in centivolts times the 4.3 divider ratio a slope of 1024 stands for.
const NUMERATOR: u32 = 473;

#[expect(
    clippy::arithmetic_side_effects,
    clippy::cast_possible_truncation,
    clippy::as_conversions,
    reason = "only ever run to build HIGH and LOW, where an overflow or the assert fails the \
              build, and From can't be called in a const fn"
)]
const fn scaled(scale: u32) -> u16 {
    let denominator = 4 * SLOPE as u32;
    let value = (NUMERATOR * scale + denominator / 2) / denominator;
    assert!(value <= u16::MAX as u32);
    value as u16
}

const HIGH: u16 = scaled(256 << 7);

const LOW: u16 = scaled(1 << 11);

const _: () = assert!(u32_from(LOW) * 0xFF <= u32_from(u16::MAX));
const _: () = {
    let top = u32_from(FULL_SCALE >> 8) * u32_from(HIGH) + ((0xFF * u32_from(LOW)) >> 4) + 64;
    assert!(top <= u32_from(u16::MAX));
};

#[expect(
    clippy::arithmetic_side_effects,
    reason = "reading is at most FULL_SCALE here, and the asserts on HIGH and LOW bound \
              every step for that"
)]
pub const fn centivolts(reading: u16) -> u16 {
    if reading > FULL_SCALE {
        return u16::MAX;
    }
    let (hi, lo) = (reading >> 8, reading & 0xFF);
    (hi * HIGH + ((lo * LOW) >> 4) + 64) >> 7
}

pub const SAMPLES_PER_SECOND: u16 = TICKS_PER_SECOND / u16_from(CYCLE_TICKS);

const _: () = assert!(TICKS_PER_SECOND.is_multiple_of(u16_from(CYCLE_TICKS)));

// Centivolts, samples and ticks from here down are all unmeasured on this torch.
pub const STEP_DOWN_BELOW: u16 = 300;
pub const OFF_BELOW: u16 = 290;
pub const RECOVER_AT: u16 = 330;
pub const CHARGED_AT: u16 = 360;
/// 10 MHz wants VDD at 2.7 V or more (DS40002204A table 36-3).
pub const CUT_BELOW: u16 = 280;
pub const CUT_SAMPLES: u8 = 2;
pub const LIGHT_FROM: u16 = 300;
/// From the MP3432's datasheet.
const BOOST_START: u16 = 270;

const _: () = assert!(BOOST_START < CUT_BELOW && CUT_BELOW < OFF_BELOW);
const _: () = assert!(CUT_BELOW < LIGHT_FROM && LIGHT_FROM < CHARGED_AT);
const _: () = assert!(OFF_BELOW < STEP_DOWN_BELOW && STEP_DOWN_BELOW < RECOVER_AT);
const _: () = assert!(RECOVER_AT < CHARGED_AT && CHARGED_AT < *PLAUSIBLE.end());

pub const HOLD_SAMPLES: u16 = SAMPLES_PER_SECOND;
pub const STEP_SAMPLES: u16 = SAMPLES_PER_SECOND * 5;
pub const RECOVER_SAMPLES: u16 = SAMPLES_PER_SECOND * 4;
pub const STEP: u8 = 15;
pub const FLAT_STEP: u8 = 30;
pub const FORGET_TICKS: u16 = TICKS_PER_SECOND * 60 * 5;
pub const PLAUSIBLE: core::ops::RangeInclusive<u16> = 100..=440;

pub fn plausible(centivolts: u16) -> bool {
    PLAUSIBLE.contains(&centivolts)
}

const _: () = assert!(
    u32_from(*PLAUSIBLE.end()) * 18 <= u32_from(u16::MAX),
    "the filter holds 16 times a sample and adds twice another"
);

pub const GLITCHES: u8 = 3;
const DOWN_EVERY: u8 = 1;
const UP_EVERY: u8 = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    Ceiling(u8),
    Flat,
    Blind,
}

#[derive(Clone, Debug)]
pub struct Protection {
    filtered: u16,
    low_samples: u16,
    flat_samples: u16,
    high_samples: u16,
    cut_samples: u8,
    implausible: u8,
    flat: bool,
    weak: bool,
    ceiling: Ceiling,
    rested: u16,
}

impl Default for Protection {
    fn default() -> Self {
        Self::new()
    }
}

impl Protection {
    pub const fn new() -> Self {
        Self {
            filtered: 0,
            low_samples: 0,
            flat_samples: 0,
            high_samples: 0,
            cut_samples: 0,
            implausible: 0,
            flat: false,
            weak: false,
            ceiling: Ceiling::new(DOWN_EVERY, UP_EVERY),
            rested: 0,
        }
    }

    pub const fn ceiling(&self) -> u8 {
        self.ceiling.level()
    }

    pub const fn flat(&self) -> bool {
        self.flat
    }

    pub fn fail(&mut self, level: u8) {
        self.ceiling.set(1, level);
    }

    pub const fn weak(&self) -> bool {
        self.weak
    }

    pub const fn unloaded(&mut self, centivolts: u16) {
        self.weak = centivolts < LIGHT_FROM;
    }

    pub const fn turn_on(&mut self) {
        self.flat |= self.weak;
    }

    pub const fn tick(&mut self) -> u8 {
        self.ceiling.tick()
    }

    pub const fn rest(&mut self, ticks: u16) {
        (self.low_samples, self.flat_samples, self.high_samples) = (0, 0, 0);
        self.implausible = 0;
        self.rested = self.rested.saturating_add(ticks);
        if self.rested >= FORGET_TICKS {
            self.filtered = 0;
        }
    }

    pub const fn centivolts(&self) -> Option<u16> {
        match self.filtered {
            0 => None,
            filtered => Some(filtered >> 4),
        }
    }

    pub fn sample(&mut self, centivolts: u16, level: u8) -> Verdict {
        let verdict = self.judge(centivolts, level);
        if let Verdict::Ceiling(target) = verdict {
            self.ceiling.set(target, level);
        }
        verdict
    }

    fn judge(&mut self, centivolts: u16, level: u8) -> Verdict {
        self.rested = 0;
        let mut ceiling = self.ceiling.target();
        if !plausible(centivolts) {
            self.high_samples = 0;
            self.implausible = self.implausible.saturating_add(1);
            if self.implausible > GLITCHES {
                return Verdict::Blind;
            }
            return Verdict::Ceiling(ceiling);
        }
        self.implausible = 0;
        if self.flat {
            if level != 0 || centivolts < CHARGED_AT {
                return Verdict::Ceiling(ceiling);
            }
            self.flat = false;
            self.filtered = 0;
            self.high_samples = 0;
            self.ceiling = Ceiling::new(DOWN_EVERY, UP_EVERY);
            ceiling = CEILING;
        }
        let voltage = self.filter(centivolts);

        if voltage >= RECOVER_AT {
            self.high_samples = self.high_samples.saturating_add(1);
            if self.high_samples >= RECOVER_SAMPLES {
                self.high_samples = 0;
                ceiling = ceiling.saturating_add(STEP).min(CEILING);
            }
        } else {
            self.high_samples = 0;
        }

        if level == 0 {
            (self.low_samples, self.flat_samples) = (0, 0);
            self.cut_samples = 0;
            return Verdict::Ceiling(ceiling);
        }
        self.cut_samples = match centivolts < CUT_BELOW {
            true => self.cut_samples.saturating_add(1),
            false => 0,
        };
        if self.cut_samples >= CUT_SAMPLES {
            (self.low_samples, self.flat_samples) = (0, 0);
            self.cut_samples = 0;
            self.flat = true;
            return Verdict::Flat;
        }
        self.low_samples = match voltage < STEP_DOWN_BELOW {
            true => self.low_samples.saturating_add(1),
            false => 0,
        };
        self.flat_samples = match voltage < OFF_BELOW {
            true if level > ceiling => 0,
            true => self.flat_samples.saturating_add(1),
            false => 0,
        };

        let drop = if self.flat_samples >= HOLD_SAMPLES {
            FLAT_STEP
        } else if self.low_samples >= STEP_SAMPLES {
            STEP
        } else {
            return Verdict::Ceiling(ceiling);
        };
        (self.low_samples, self.flat_samples) = (0, 0);
        let from = ceiling.min(level);
        if from == 1 {
            self.flat = true;
            return Verdict::Flat;
        }
        Verdict::Ceiling(from.saturating_sub(drop).max(1))
    }

    #[expect(
        clippy::arithmetic_side_effects,
        reason = "an eighth of filtered is never more than filtered"
    )]
    fn filter(&mut self, centivolts: u16) -> u16 {
        self.filtered = match self.filtered {
            0 => centivolts << 4,
            filtered => (filtered - (filtered >> 3))
                .checked_add(centivolts << 1)
                .unwrap_or(0),
        };
        self.filtered >> 4
    }
}

#[cfg(test)]
mod tests {
    extern crate std;

    use super::*;
    use std::vec::Vec;

    fn exact(reading: u16) -> f64 {
        let pin = 1.1 * f64::from(reading) / (4.0 * 1024.0);
        pin * 4.3 * 1024.0 / f64::from(SLOPE) * 100.0
    }

    #[test]
    fn matches_the_formula_for_every_reading_the_adc_can_give() {
        for reading in 0..=FULL_SCALE {
            let error = (f64::from(centivolts(reading)) - exact(reading)).abs();
            assert!(error < 0.6, "{reading} is off by {error}");
        }
    }

    #[test]
    fn rises_with_the_reading() {
        for reading in 0..FULL_SCALE {
            assert!(centivolts(reading) <= centivolts(reading + 1), "{reading}");
        }
    }

    #[test]
    fn the_effective_ratio_is_about_4_03() {
        let ratio = 4.3 * 1024.0 / f64::from(SLOPE);
        assert!((ratio - 4.03).abs() < 0.01, "{ratio}");
    }

    #[test]
    fn known_points() {
        assert_eq!(centivolts(0), 0);
        assert_eq!(centivolts(2775), 300);
        assert_eq!(centivolts(3886), 420);
        assert_eq!(centivolts(FULL_SCALE), 442);
    }

    #[test]
    fn a_reading_the_adc_cant_give_is_safe_rather_than_an_overflow() {
        assert_eq!(centivolts(FULL_SCALE + 1), u16::MAX);
        assert_eq!(centivolts(u16::MAX), u16::MAX);
    }

    #[test]
    fn samples_come_four_times_a_second() {
        assert_eq!(SAMPLES_PER_SECOND, 4);
    }

    const fn secs(seconds: u16) -> u16 {
        seconds * SAMPLES_PER_SECOND
    }

    fn run(lvp: &mut Protection, cv: u16, level: u8, samples: u16) -> Vec<Verdict> {
        (0..samples).map(|_| lvp.sample(cv, level)).collect()
    }

    fn last(lvp: &mut Protection, cv: u16, level: u8, samples: u16) -> Verdict {
        *run(lvp, cv, level, samples).last().unwrap()
    }

    fn all(verdicts: &[Verdict], expected: Verdict) -> bool {
        verdicts.iter().all(|&v| v == expected)
    }

    fn follow(lvp: &mut Protection, cv: u16, level: u8, samples: u16) -> Vec<Verdict> {
        let mut at = level;
        (0..samples)
            .map(|_| {
                let verdict = lvp.sample(cv, at);
                if let Verdict::Ceiling(ceiling) = verdict {
                    at = level.min(ceiling);
                }
                verdict
            })
            .collect()
    }

    const NONE: Verdict = Verdict::Ceiling(CEILING);

    #[test]
    fn a_full_cell_has_no_ceiling() {
        let mut lvp = Protection::new();
        assert_eq!(lvp.centivolts(), None);
        assert!(all(&run(&mut lvp, 410, CEILING, secs(600)), NONE));
        assert_eq!(lvp.centivolts(), Some(410));
    }

    #[test]
    fn the_first_sample_seeds_the_filter() {
        let mut lvp = Protection::new();
        lvp.sample(290, 100);
        assert_eq!(lvp.centivolts(), Some(290));
    }

    #[test]
    fn steps_down_after_5_seconds_below_3_volts() {
        let mut lvp = Protection::new();
        let verdicts = run(&mut lvp, 295, 100, secs(5));
        let (before, after) = verdicts.split_at(usize::from(secs(5)) - 1);
        assert!(all(before, NONE));
        assert_eq!(after, [Verdict::Ceiling(100 - STEP)]);
    }

    #[test]
    fn exactly_3_volts_is_not_low() {
        let mut lvp = Protection::new();
        assert!(all(&run(&mut lvp, STEP_DOWN_BELOW, 100, secs(600)), NONE));
    }

    #[test]
    fn keeps_stepping_down_every_5_seconds_then_turns_off() {
        let mut lvp = Protection::new();
        for ceiling in [85, 70, 55, 40, 25, 10, 1] {
            assert_eq!(last(&mut lvp, 295, 100, secs(5)), Verdict::Ceiling(ceiling));
        }
        assert_eq!(last(&mut lvp, 295, 100, secs(5)), Verdict::Flat);
    }

    #[test]
    fn steps_from_the_level_when_it_is_under_the_ceiling() {
        let mut lvp = Protection::new();
        assert_eq!(last(&mut lvp, 295, 40, secs(5)), Verdict::Ceiling(25));
    }

    #[test]
    fn a_second_below_2_9_volts_drops_a_big_step_rather_than_turning_off() {
        let mut lvp = Protection::new();
        let verdicts = run(&mut lvp, 285, 100, secs(1));
        let (before, after) = verdicts.split_at(usize::from(secs(1)) - 1);
        assert!(all(before, NONE));
        assert_eq!(after, [Verdict::Ceiling(100 - FLAT_STEP)]);
    }

    #[test]
    fn turns_off_after_a_second_below_2_9_volts_at_level_1() {
        let mut lvp = Protection::new();
        let verdicts = run(&mut lvp, 285, 1, secs(1));
        let (before, after) = verdicts.split_at(usize::from(secs(1)) - 1);
        assert!(all(before, NONE));
        assert_eq!(after, [Verdict::Flat]);
    }

    #[test]
    fn two_lit_samples_below_2_8_volts_cut_it_at_any_level() {
        for level in [1, 50, CEILING] {
            let mut lvp = Protection::new();
            run(&mut lvp, 380, level, secs(10));
            assert_eq!(lvp.sample(CUT_BELOW - 1, level), NONE, "{level}");
            assert_eq!(lvp.sample(CUT_BELOW - 1, level), Verdict::Flat, "{level}");
            assert!(lvp.flat());
        }
    }

    #[test]
    fn one_sample_below_2_8_volts_is_no_cut() {
        let mut lvp = Protection::new();
        for _ in 0..secs(60) {
            assert_eq!(lvp.sample(CUT_BELOW - 1, 50), NONE);
            assert_eq!(lvp.sample(380, 50), NONE);
        }
        assert!(all(&run(&mut lvp, CUT_BELOW, 50, 2), NONE));
    }

    #[test]
    fn a_sample_with_the_light_off_breaks_the_run_to_a_cut() {
        let mut lvp = Protection::new();
        assert_eq!(lvp.sample(CUT_BELOW - 1, 50), NONE);
        lvp.sample(CUT_BELOW - 1, 0);
        assert_eq!(lvp.sample(CUT_BELOW - 1, 50), NONE);
        assert_eq!(lvp.sample(CUT_BELOW - 1, 50), Verdict::Flat);
    }

    #[test]
    fn turning_on_after_a_weak_unloaded_reading_is_flat() {
        let mut lvp = Protection::new();
        lvp.turn_on();
        assert!(!lvp.flat(), "nothing read yet");
        lvp.unloaded(LIGHT_FROM);
        lvp.turn_on();
        assert!(!lvp.flat());
        lvp.unloaded(LIGHT_FROM - 1);
        assert!(!lvp.flat(), "only once it turns on");
        lvp.turn_on();
        assert!(lvp.flat());
        assert_eq!(lvp.sample(LIGHT_FROM, 0), Verdict::Ceiling(CEILING));
        assert!(lvp.flat(), "until charged");
        lvp.unloaded(CHARGED_AT);
        lvp.sample(CHARGED_AT, 0);
        lvp.turn_on();
        assert!(!lvp.flat());
    }

    #[test]
    fn a_flat_cell_goes_down_the_ramp_in_big_steps_before_turning_off() {
        let mut lvp = Protection::new();
        let mut verdicts = follow(&mut lvp, 280, CEILING, secs(10));
        verdicts.dedup();
        let off = verdicts.iter().position(|&v| v == Verdict::Flat).unwrap();
        assert_eq!(
            verdicts[..=off],
            [
                NONE,
                Verdict::Ceiling(120),
                Verdict::Ceiling(90),
                Verdict::Ceiling(60),
                Verdict::Ceiling(30),
                Verdict::Ceiling(1),
                Verdict::Flat
            ]
        );
    }

    #[test]
    fn a_cell_that_recovers_under_the_lighter_load_stays_on() {
        let mut lvp = Protection::new();
        assert_eq!(last(&mut lvp, 280, CEILING, secs(1)), Verdict::Ceiling(120));
        let held = Verdict::Ceiling(120);
        assert!(all(&run(&mut lvp, 315, 120, secs(60)), held));
    }

    #[test]
    fn waits_for_the_light_to_reach_the_ceiling_before_judging_it() {
        let mut lvp = Protection::new();
        assert_eq!(last(&mut lvp, 285, CEILING, secs(1)), Verdict::Ceiling(120));
        let held = Verdict::Ceiling(120);
        assert!(all(&run(&mut lvp, 285, 121, secs(4)), held));
        let verdicts = run(&mut lvp, 285, 120, secs(1));
        let (before, after) = verdicts.split_at(usize::from(secs(1)) - 1);
        assert!(all(before, held));
        assert_eq!(after, [Verdict::Ceiling(90)]);
    }

    #[test]
    fn exactly_2_9_volts_only_takes_the_slow_steps() {
        let mut lvp = Protection::new();
        let verdicts = run(&mut lvp, OFF_BELOW, 50, secs(5));
        assert!(all(&verdicts[..usize::from(secs(5)) - 1], NONE));
        assert_eq!(verdicts.last(), Some(&Verdict::Ceiling(35)));
    }

    #[test]
    fn one_bad_reading_does_nothing() {
        let mut lvp = Protection::new();
        run(&mut lvp, 380, 50, secs(10));
        for glitch in [0, 1, 50, 289] {
            assert_eq!(lvp.sample(glitch, 50), NONE);
            assert!(all(&run(&mut lvp, 380, 50, secs(60)), NONE));
        }
    }

    #[test]
    fn half_a_second_of_zero_readings_does_nothing() {
        let mut lvp = Protection::new();
        run(&mut lvp, 380, 50, secs(10));
        assert!(all(&run(&mut lvp, 0, 50, secs(1) / 2), NONE));
        assert!(all(&run(&mut lvp, 380, 50, secs(60)), NONE));
    }

    #[test]
    fn the_filter_takes_a_couple_of_seconds_to_follow_a_drop() {
        let mut lvp = Protection::new();
        run(&mut lvp, 360, 50, secs(10));
        let verdicts = run(&mut lvp, CUT_BELOW, 50, secs(10));
        let dropped = verdicts.iter().position(|&v| v != NONE).unwrap();
        let dropped = u16::try_from(dropped).unwrap();
        assert!(dropped > secs(4) && dropped < secs(6), "{dropped}");
    }

    #[test]
    fn a_voltage_back_above_3_volts_restarts_the_hold() {
        let mut lvp = Protection::new();
        run(&mut lvp, 295, 100, secs(3));
        run(&mut lvp, 310, 100, secs(2));
        assert!(all(&run(&mut lvp, 295, 100, secs(5) - 1), NONE));
        assert!(run(&mut lvp, 295, 100, secs(5)).contains(&Verdict::Ceiling(85)));
    }

    #[test]
    fn does_nothing_while_the_torch_is_off() {
        let mut lvp = Protection::new();
        assert!(all(&run(&mut lvp, 250, 0, secs(600)), NONE));
        assert_eq!(last(&mut lvp, 285, 1, secs(1) - 1), NONE);
        assert_eq!(lvp.sample(285, 1), Verdict::Flat);
    }

    #[test]
    fn turning_off_keeps_the_ceiling() {
        let mut lvp = Protection::new();
        assert_eq!(last(&mut lvp, 295, 16, secs(5)), Verdict::Ceiling(1));
        assert!(run(&mut lvp, 250, 1, secs(5)).contains(&Verdict::Flat));
        assert_eq!(last(&mut lvp, 280, 0, secs(1)), Verdict::Ceiling(1));
    }

    #[test]
    fn a_long_rest_keeps_the_ceiling_and_forgets_the_filter() {
        let mut lvp = Protection::new();
        assert_eq!(last(&mut lvp, 295, 60, secs(5)), Verdict::Ceiling(45));
        for _ in 0..FORGET_TICKS {
            lvp.rest(1);
        }
        assert_eq!(lvp.centivolts(), None);
        assert_eq!(lvp.sample(380, 0), Verdict::Ceiling(45));
        assert_eq!(lvp.centivolts(), Some(380));
    }

    #[test]
    fn the_filter_forgets_the_cell_after_five_minutes_of_rest() {
        assert_eq!(TICKS_PER_SECOND, 64);
        assert_eq!(FORGET_TICKS, 19_200);
        assert_eq!(FORGET_TICKS / TICKS_PER_SECOND, 5 * 60);
    }

    #[test]
    fn a_short_rest_keeps_the_filter() {
        let mut lvp = Protection::new();
        run(&mut lvp, 280, 50, secs(1) - 1);
        for _ in 0..FORGET_TICKS - 1 {
            lvp.rest(1);
        }
        assert_eq!(lvp.centivolts(), Some(280));
    }

    #[test]
    fn rest_ticks_of_a_second_forget_in_five_minutes_too() {
        let mut lvp = Protection::new();
        run(&mut lvp, 280, 50, secs(1) - 1);
        for _ in 1..FORGET_TICKS / TICKS_PER_SECOND {
            lvp.rest(TICKS_PER_SECOND);
        }
        assert_eq!(lvp.centivolts(), Some(280));
        lvp.rest(TICKS_PER_SECOND);
        assert_eq!(lvp.centivolts(), None);
    }

    #[test]
    fn a_sample_starts_the_wait_to_forget_again() {
        let mut lvp = Protection::new();
        for _ in 0..3 {
            for _ in 0..FORGET_TICKS - 1 {
                lvp.rest(1);
            }
            lvp.sample(380, 0);
        }
        assert_eq!(lvp.centivolts(), Some(380));
    }

    #[test]
    fn a_rest_starts_the_hold_afresh() {
        let mut lvp = Protection::new();
        run(&mut lvp, 285, 1, secs(1) - 1);
        lvp.rest(1);
        assert_eq!(last(&mut lvp, 285, 1, secs(1) - 1), NONE);
        assert_eq!(lvp.sample(285, 1), Verdict::Flat);
    }

    #[test]
    fn hysteresis_holds_the_ceiling_between_3_and_3_3_volts() {
        let mut lvp = Protection::new();
        let resting = Verdict::Ceiling(CEILING - STEP);
        assert_eq!(last(&mut lvp, 295, CEILING, secs(5)), resting);
        assert!(all(&run(&mut lvp, 329, CEILING - STEP, secs(600)), resting));
        assert!(all(&run(&mut lvp, 300, CEILING - STEP, secs(600)), resting));
    }

    #[test]
    fn recovers_a_step_every_4_seconds_at_3_3_volts() {
        let mut lvp = Protection::new();
        assert_eq!(last(&mut lvp, 295, 50, secs(5)), Verdict::Ceiling(35));
        let verdicts = run(&mut lvp, 340, 35, secs(8));
        let up = verdicts
            .iter()
            .position(|&v| v == Verdict::Ceiling(50))
            .unwrap();
        let up = u16::try_from(up).unwrap();
        assert!(up >= secs(4) && up < secs(8), "{up}");
        assert_eq!(last(&mut lvp, 340, 0, secs(4)), Verdict::Ceiling(65));
        assert_eq!(last(&mut lvp, 340, 0, secs(120)), NONE);
    }

    #[test]
    fn the_filter_has_room_for_the_most_plausible_voltage() {
        let mut lvp = Protection::new();
        assert!(u16::try_from(u32::from(*PLAUSIBLE.end()) << 4).is_ok());
        assert_eq!(last(&mut lvp, *PLAUSIBLE.end(), CEILING, secs(60)), NONE);
        assert_eq!(lvp.centivolts(), Some(*PLAUSIBLE.end()));
    }

    #[test]
    fn plausible_covers_a_real_cell_and_nothing_broken() {
        for cv in [100, 250, 290, 310, 420, 435] {
            assert!(plausible(cv), "{cv}");
        }
        for cv in [
            centivolts(0),
            centivolts(FULL_SCALE),
            centivolts(u16::MAX),
            99,
            441,
        ] {
            assert!(!plausible(cv), "{cv}");
        }
    }

    #[test]
    fn a_cell_collapsing_under_load_counts_as_flat_rather_than_garbage() {
        let mut lvp = Protection::new();
        run(&mut lvp, 300, 90, secs(1));
        let verdicts = follow(&mut lvp, 150, 90, secs(12));
        assert!(verdicts.contains(&Verdict::Flat));
    }

    #[test]
    fn implausible_samples_never_reach_the_filter() {
        let mut lvp = Protection::new();
        lvp.sample(u16::MAX, 100);
        lvp.sample(0, 100);
        assert_eq!(lvp.centivolts(), None);
        lvp.sample(400, 100);
        lvp.sample(u16::MAX, 100);
        assert_eq!(lvp.centivolts(), Some(400));
    }

    #[test]
    fn a_second_of_garbage_is_blind_on_or_off() {
        for level in [0, 1, 30, CEILING] {
            for garbage in [0, 99, 441, centivolts(FULL_SCALE), u16::MAX] {
                let mut lvp = Protection::new();
                let verdicts = run(&mut lvp, garbage, level, u16::from(GLITCHES));
                assert!(all(&verdicts, NONE), "{garbage} at level {level}");
                let verdicts = run(&mut lvp, garbage, level, secs(10));
                assert!(all(&verdicts, Verdict::Blind), "{garbage} at {level}");
            }
        }
    }

    #[test]
    fn a_plausible_sample_starts_the_count_to_blind_again() {
        for good in [*PLAUSIBLE.start(), 310, *PLAUSIBLE.end()] {
            let mut lvp = Protection::new();
            run(&mut lvp, 0, CEILING, u16::from(GLITCHES));
            lvp.sample(good, CEILING);
            assert!(all(&run(&mut lvp, 0, CEILING, u16::from(GLITCHES)), NONE));
            assert_eq!(lvp.sample(0, CEILING), Verdict::Blind, "{good}");
        }
    }

    #[test]
    fn a_rest_starts_the_count_to_blind_again() {
        let mut lvp = Protection::new();
        run(&mut lvp, 0, 0, u16::from(GLITCHES));
        lvp.rest(1);
        assert!(all(&run(&mut lvp, 0, 0, u16::from(GLITCHES)), NONE));
        assert_eq!(lvp.sample(0, 0), Verdict::Blind);
    }

    #[test]
    fn a_glitch_or_three_keeps_a_lower_ceiling() {
        let mut lvp = Protection::new();
        assert_eq!(last(&mut lvp, 295, 40, secs(5)), Verdict::Ceiling(25));
        assert_eq!(
            last(&mut lvp, 0, 25, u16::from(GLITCHES)),
            Verdict::Ceiling(25)
        );
    }

    fn flat() -> Protection {
        let mut lvp = Protection::new();
        assert_eq!(last(&mut lvp, 285, 1, secs(1)), Verdict::Flat);
        assert!(lvp.flat());
        lvp
    }

    #[test]
    fn flat_stays_flat_until_a_reading_with_the_light_off_says_its_charged() {
        let mut lvp = flat();
        let quiet =
            |verdicts: Vec<Verdict>| verdicts.iter().all(|v| matches!(v, Verdict::Ceiling(_)));
        assert!(quiet(run(&mut lvp, CHARGED_AT - 1, 0, secs(600))));
        assert!(lvp.flat());
        assert!(quiet(run(&mut lvp, 400, 1, secs(10))));
        assert!(lvp.flat());
        assert_eq!(lvp.sample(CHARGED_AT, 0), NONE);
        assert!(!lvp.flat());
        assert_eq!(lvp.ceiling(), CEILING);
        assert_eq!(lvp.centivolts(), Some(CHARGED_AT));
    }

    #[test]
    fn a_rest_keeps_it_flat() {
        let mut lvp = flat();
        for _ in 0..FORGET_TICKS {
            lvp.rest(1);
        }
        assert!(lvp.flat());
    }

    #[test]
    fn a_charged_cell_that_runs_flat_again_turns_off_again() {
        let mut lvp = flat();
        lvp.sample(400, 0);
        assert!(!lvp.flat());
        assert!(follow(&mut lvp, 250, CEILING, secs(30)).contains(&Verdict::Flat));
        assert!(lvp.flat());
    }

    #[test]
    fn fail_fades_the_ceiling_to_level_1() {
        let mut lvp = Protection::new();
        lvp.fail(50);
        assert_eq!(lvp.ceiling(), 50, "from the level the light's at");
        for _ in 0..49 {
            lvp.tick();
        }
        assert_eq!(lvp.ceiling(), 1);
    }

    #[test]
    fn garbage_never_counts_towards_stepping_up() {
        let mut lvp = Protection::new();
        assert_eq!(last(&mut lvp, 295, 50, secs(5)), Verdict::Ceiling(35));
        run(&mut lvp, 340, 35, secs(3));
        for _ in 0..200 {
            lvp.sample(340, 35);
            lvp.sample(340, 35);
            assert_eq!(lvp.sample(0, 35), Verdict::Ceiling(35));
        }
    }

    #[test]
    fn garbage_doesnt_restart_the_hold_on_a_flat_cell() {
        let mut lvp = Protection::new();
        let mut verdicts = Vec::new();
        for _ in 0..secs(2) {
            verdicts.push(lvp.sample(280, 1));
            verdicts.push(lvp.sample(u16::MAX, 1));
        }
        assert!(verdicts.contains(&Verdict::Flat));
    }

    #[derive(Debug, PartialEq)]
    enum Held {
        Fine,
        Flat,
        Cut,
        Unreadable,
    }

    fn held(cv: u16) -> Held {
        if !PLAUSIBLE.contains(&cv) {
            Held::Unreadable
        } else if cv < CUT_BELOW {
            Held::Cut
        } else if cv < STEP_DOWN_BELOW {
            Held::Flat
        } else {
            Held::Fine
        }
    }

    #[test]
    fn every_reading_held_ends_where_the_thresholds_say() {
        let mut ends = [0_u32; 4];
        for raw in 0..=FULL_SCALE {
            let cv = centivolts(raw);
            let expected = held(cv);
            let mut lvp = Protection::new();
            let mut level = CEILING;
            let mut off_at = None;
            for n in 1..=secs(600) {
                let verdict = lvp.sample(cv, level);
                for _ in 0..CYCLE_TICKS {
                    lvp.tick();
                }
                level = CEILING.min(lvp.ceiling());
                if let Verdict::Ceiling(ceiling) = verdict {
                    assert!(ceiling <= CEILING, "raw {raw}, sample {n}: {ceiling}");
                }
                assert!(lvp.ceiling() <= CEILING, "raw {raw}, sample {n}");
                let want = match expected {
                    Held::Fine => Some(NONE),
                    Held::Unreadable if n <= u16::from(GLITCHES) => Some(NONE),
                    Held::Unreadable => Some(Verdict::Blind),
                    Held::Cut if n < u16::from(CUT_SAMPLES) => Some(NONE),
                    Held::Cut => Some(Verdict::Flat),
                    Held::Flat => None,
                };
                if let Some(want) = want {
                    assert_eq!(verdict, want, "raw {raw} ({cv} cV), sample {n}");
                }
                assert_eq!(lvp.flat(), verdict == Verdict::Flat, "raw {raw}, {n}");
                if matches!(verdict, Verdict::Flat | Verdict::Blind) {
                    off_at = Some(n);
                    break;
                }
            }
            match expected {
                Held::Fine => assert_eq!(off_at, None, "raw {raw} ({cv} cV)"),
                Held::Flat => {
                    let at = off_at.unwrap_or_else(|| panic!("raw {raw} ({cv} cV) never off"));
                    assert!(at >= HOLD_SAMPLES, "raw {raw} ({cv} cV) off at {at}");
                }
                Held::Unreadable => {
                    let blind = u16::from(GLITCHES) + 1;
                    assert_eq!(off_at, Some(blind), "raw {raw} ({cv} cV)");
                }
                Held::Cut => {
                    let cut = u16::from(CUT_SAMPLES);
                    assert_eq!(off_at, Some(cut), "raw {raw} ({cv} cV)");
                }
            }
            ends[match expected {
                Held::Fine => 0,
                Held::Flat => 1,
                Held::Cut => 2,
                Held::Unreadable => 3,
            }] += 1;
        }
        assert!(ends.iter().all(|&count| count > 0), "{ends:?}");
    }
}
