//! The cell's voltage, and what to do as it runs flat.
//!
//! The MCU runs from a regulator, so unlike the TS21 its own supply says
//! nothing about the cell. The cell comes in on PA1 (AIN1) through a divider,
//! and the ADC reads that against the internal 1.1 V reference, four
//! conversions accumulated:
//!
//! ```text
//! pin  = 1.1 V × reading / (4 × 1024)
//! cell = pin × 4.3 × 1024 / SLOPE
//! ```
//!
//! which in centivolts comes to `reading × 473 / (4 × SLOPE)`. Those are the
//! facts of `mcu_vdivider_raw2cooked16` in Anduril's `arch/attiny1616.c` and
//! `VOLTAGE_SLOPE` in `hw/wurkkos/ts26/hwdef.h`. The effective ratio is
//! 4.3 × 1024 / 1094, about 4.03, so the ADC runs out at about 4.42 V
//! (docs/hardware.md).
//!
//! `reading × 473` needs 21 bits, and there's no runtime 32-bit multiply on
//! this chip (docs/toolchain.md). So this splits the reading into its high
//! and low bytes, each multiplied by a constant worked out at compile time
//! that's scaled to keep the product in a `u16`:
//!
//! ```text
//! centivolts = (hi × HIGH + ((lo × LOW) >> 4) + 64) >> 7
//! ```
//!
//! That's in 128ths of a centivolt until the last shift. The two rounded
//! constants cost under a tenth of a centivolt, which next to a calibration
//! from one sample is nothing.
//!
//! The thresholds are this board's, not the TS21's. The Wurkkos 21700 is
//! unprotected, and the MP3432 keeps running down to 0.8 V on its input once
//! it's started, pulling more current as the voltage falls, so nothing but
//! this module stops the cell being run into the ground.

use crate::TICKS_PER_SECOND;
use crate::limits::FAULT_CEILING;
use crate::ramp::CEILING;

/// Anduril's `VOLTAGE_SLOPE` for this board, against its default of 1024,
/// higher reading lower. Calibrated on one of ToyKeeper's samples, and
/// unmeasured on any light of ours. `VOLTAGE_OFFSET` is 0 here, so there's no
/// offset to carry.
pub const SLOPE: u16 = 1094;

/// 1.1 V in centivolts times the 4.3 that a slope of 1024 stands for.
const NUMERATOR: u32 = 473;

/// Four full-scale 10-bit conversions, the most the ADC can give.
const MAX_READING: u16 = 4 * 1023;

/// `NUMERATOR / (4 × SLOPE)` centivolts a count, times `scale`, rounded.
const fn scaled(scale: u32) -> u16 {
    let denominator = 4 * SLOPE as u32;
    ((NUMERATOR * scale + denominator / 2) / denominator) as u16
}

/// 256 counts in 128ths of a centivolt.
const HIGH: u16 = scaled(256 << 7);

/// One count in 2048ths of a centivolt, sixteen times finer than [`HIGH`]
/// because it's a small number that needs the digits.
const LOW: u16 = scaled(1 << 11);

const _: () = assert!(LOW as u32 * 0xFF <= u16::MAX as u32);
const _: () = {
    let top = (MAX_READING >> 8) as u32 * HIGH as u32 + ((0xFF * LOW as u32) >> 4) + 64;
    assert!(top <= u16::MAX as u32);
};

/// The cell's voltage in centivolts from one accumulated reading of AIN1,
/// rounded to the nearest centivolt.
///
/// A reading over [`MAX_READING`] can't come from the ADC, so it comes out as
/// `u16::MAX`, and zero comes out as zero. Neither is [`plausible`], so
/// [`Protection`] treats them as the garbage they are.
pub fn centivolts(reading: u16) -> u16 {
    if reading > MAX_READING {
        return u16::MAX;
    }
    let (hi, lo) = (reading >> 8, reading & 0xFF);
    (hi * HIGH + ((lo * LOW) >> 4) + 64) >> 7
}

/// How many ticks apart the voltage samples come. The ADC alternates between
/// the voltage and the temperature sensor every 8 ticks, so each gets a fresh
/// reading every 16.
pub const SAMPLE_TICKS: u16 = 16;

/// Voltage samples a second: four.
pub const SAMPLES_PER_SECOND: u16 = TICKS_PER_SECOND / SAMPLE_TICKS;

const _: () = assert!(TICKS_PER_SECOND.is_multiple_of(SAMPLE_TICKS));

/// Below this many centivolts, held for [`STEP_SAMPLES`], the ceiling steps
/// down. It's a tenth of a volt over [`OFF_BELOW`], so the light starts
/// dimming before the cell gets there. Unmeasured.
pub const STEP_DOWN_BELOW: u16 = 300;

/// Below this many centivolts, held for [`HOLD_SAMPLES`], the ceiling drops
/// [`FLAT_STEP`] levels, and at level 1 the torch turns off.
///
/// It's Anduril's default `VOLTAGE_LOW` (`fsm/adc.h`), which the TS26's build
/// doesn't override. Wurkkos quotes 2.8 V, but with an unprotected cell the
/// higher figure is the one to have. It also leaves 0.2 V, about 7%, over the
/// MP3432's 2.7 V start-up, so a light that's cut out can start again, and
/// that 7% has to cover the 1.1 V reference's tolerance and a divider ratio
/// nobody's checked. If the ratio's really the 4.3 of the old comment, this
/// reads low and cuts out early, which is the safe way to be wrong.
/// Unmeasured.
pub const OFF_BELOW: u16 = 290;

/// At or above this many centivolts, held for [`RECOVER_SAMPLES`], the
/// ceiling steps back up. The 0.3 V gap to [`STEP_DOWN_BELOW`] is the
/// hysteresis: a cell that sags to 2.95 V at a high level and rests at 3.2 V
/// on a lower one stays put. Unmeasured.
pub const RECOVER_AT: u16 = 330;

/// The MP3432's start-up voltage from its datasheet, in centivolts.
const BOOST_START: u16 = 270;

const _: () = assert!(BOOST_START < OFF_BELOW);
const _: () = assert!(OFF_BELOW < STEP_DOWN_BELOW && STEP_DOWN_BELOW < RECOVER_AT);

/// How long the filtered voltage has to stay under [`OFF_BELOW`] before it
/// acts: one second. Unmeasured.
pub const HOLD_SAMPLES: u16 = SAMPLES_PER_SECOND;

/// How long it has to stay under [`STEP_DOWN_BELOW`] for each step down: five
/// seconds. Long enough for the cell to recover a little after the last step,
/// the way Anduril waits about six seconds between its low-voltage
/// step-downs. Unmeasured.
pub const STEP_SAMPLES: u16 = SAMPLES_PER_SECOND * 5;

/// How long it has to stay at [`RECOVER_AT`] or above for each step back up:
/// four seconds. Unmeasured.
pub const RECOVER_SAMPLES: u16 = SAMPLES_PER_SECOND * 4;

/// How many levels each step down (or up) moves the ceiling, a tenth of the
/// ramp. Unmeasured.
pub const STEP: u8 = 15;

/// How many levels the ceiling drops for each hold under [`OFF_BELOW`], a
/// fifth of the ramp. From the top of the ramp that's four drops and then
/// off, around seven seconds with the fades. Unmeasured.
pub const FLAT_STEP: u8 = 30;

/// Ticks of rest before the filter forgets the cell, five minutes. Any
/// sooner and clicking off and on would restart it from the unloaded voltage
/// each time, which keeps a nearly flat cell running. Any later and a cell
/// that's sat for days gets judged on what it read before. Unmeasured.
pub const FORGET_TICKS: u16 = TICKS_PER_SECOND * 60 * 5;

/// Voltages this board could really read, in centivolts.
///
/// The top sits under the 4.42 V of four saturated conversions, with room
/// for a full cell and a calibration off by a few per cent. The bottom is
/// low on purpose. The boost converter can drag a dying cell far below
/// anything sensible, and a reading of 1.5 V under load is the worst possible
/// moment to call a real voltage garbage, so only what's left of a dead ADC
/// or a shorted pin falls under it. Unmeasured.
pub const PLAUSIBLE: core::ops::RangeInclusive<u16> = 100..=440;

/// Whether a voltage from [`centivolts`] could be a real cell.
pub fn plausible(centivolts: u16) -> bool {
    PLAUSIBLE.contains(&centivolts)
}

/// Implausible samples in a row that [`Protection`] lets go before it assumes
/// the worst. The fourth in a row, a second's worth, acts. Unmeasured.
const GLITCHES: u8 = 3;

/// The low-voltage verdict for one sample.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// Run no higher than this level. [`CEILING`] means no limit.
    Ceiling(u8),
    /// Turn off now. The cell's flat.
    Off,
}

/// Low-voltage protection: feed it each voltage sample (every
/// [`SAMPLE_TICKS`] ticks) and it says how bright the torch may be.
///
/// Samples go through a moving average (each one moves the filtered value an
/// eighth of the way, a time constant of about two seconds at four samples a
/// second), and the filtered value has to stay low for a whole second or more
/// before it acts, so one bad reading, or the sag as the light comes on, does
/// nothing.
///
/// - Under [`STEP_DOWN_BELOW`] for [`STEP_SAMPLES`], the ceiling drops to
///   [`STEP`] levels under whichever is lower of the ceiling and the level the
///   torch is at, or the torch turns off if that's already level 1.
/// - Under [`OFF_BELOW`] for [`HOLD_SAMPLES`], the same with [`FLAT_STEP`].
///   It only ever turns off from level 1, because up the ramp several amps
///   sag the cell a lot, and a low reading there may be a cell with plenty
///   left. Each hold starts again once the light's faded down to the new
///   ceiling, so each is judged on the load it's actually under.
/// - At [`RECOVER_AT`] or above for [`RECOVER_SAMPLES`], the ceiling steps
///   back up by [`STEP`].
///
/// It never steps down or turns off while the torch is off, since there's no
/// load to sag under. It can recover then, but only while the switch keeps
/// the readings coming, since nothing's read during a [`Protection::rest`].
///
/// A sample that isn't [`plausible`] never reaches the filter. It can't
/// count towards stepping back up, and it doesn't restart the holds, so a
/// flat cell can't hide behind the odd bad reading. More than [`GLITCHES`]
/// in a row and the ADC can't be trusted to say the cell's fine, so the
/// ceiling drops to [`FAULT_CEILING`], on or off. It never turns the torch
/// off, since a broken reading isn't evidence of a flat cell. Good samples
/// at [`RECOVER_AT`] or above step it back up as usual.
#[derive(Clone, Debug)]
pub struct Protection {
    /// The filtered voltage in sixteenths of a centivolt. Zero until the
    /// first plausible sample, which seeds it.
    filtered: u16,
    low_samples: u16,
    flat_samples: u16,
    high_samples: u16,
    /// Implausible samples in a row.
    implausible: u8,
    ceiling: u8,
    /// Ticks of rest since the last sample, up to [`FORGET_TICKS`].
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
            implausible: 0,
            ceiling: CEILING,
            rested: 0,
        }
    }

    /// One tick with nothing measuring the cell. The holds start again, as
    /// they do for any time off, and the ceiling stays, since only good
    /// readings raise it. The filter stays too, until [`FORGET_TICKS`] have
    /// gone by.
    pub fn rest(&mut self) {
        (self.low_samples, self.flat_samples, self.high_samples) = (0, 0, 0);
        self.implausible = 0;
        self.rested = self.rested.saturating_add(1);
        if self.rested >= FORGET_TICKS {
            self.filtered = 0;
        }
    }

    /// The filtered voltage in centivolts, or `None` before the first sample.
    pub fn centivolts(&self) -> Option<u16> {
        match self.filtered {
            0 => None,
            filtered => Some(filtered >> 4),
        }
    }

    /// One voltage sample: `centivolts` from [`centivolts`] and the level the
    /// torch is at (`0` for off).
    pub fn sample(&mut self, centivolts: u16, level: u8) -> Verdict {
        self.rested = 0;
        if !plausible(centivolts) {
            self.high_samples = 0;
            self.implausible = self.implausible.saturating_add(1);
            if self.implausible > GLITCHES {
                self.ceiling = self.ceiling.min(FAULT_CEILING);
            }
            return Verdict::Ceiling(self.ceiling);
        }
        self.implausible = 0;
        let voltage = self.filter(centivolts);

        if voltage >= RECOVER_AT {
            self.high_samples += 1;
            if self.high_samples >= RECOVER_SAMPLES {
                self.high_samples = 0;
                self.ceiling = self.ceiling.saturating_add(STEP).min(CEILING);
            }
        } else {
            self.high_samples = 0;
        }

        if level == 0 {
            (self.low_samples, self.flat_samples) = (0, 0);
            return Verdict::Ceiling(self.ceiling);
        }
        self.low_samples = match voltage < STEP_DOWN_BELOW {
            true => self.low_samples + 1,
            false => 0,
        };
        self.flat_samples = match voltage < OFF_BELOW {
            // Still fading down to the ceiling, so the cell's still under
            // the heavier load. Wait till it's there and judge it on that.
            true if level > self.ceiling => 0,
            true => self.flat_samples + 1,
            false => 0,
        };

        let drop = if self.flat_samples >= HOLD_SAMPLES {
            FLAT_STEP
        } else if self.low_samples >= STEP_SAMPLES {
            STEP
        } else {
            return Verdict::Ceiling(self.ceiling);
        };
        (self.low_samples, self.flat_samples) = (0, 0);
        let from = self.ceiling.min(level);
        if from == 1 {
            return Verdict::Off;
        }
        self.ceiling = from.saturating_sub(drop).max(1);
        Verdict::Ceiling(self.ceiling)
    }

    /// Folds a sample into the moving average and returns the result in
    /// centivolts. In sixteenths, moving an eighth of the way is taking off
    /// an eighth and adding the sample times two. Only plausible samples get
    /// here, so the sum stays well inside a `u16` and the seed isn't zero.
    fn filter(&mut self, centivolts: u16) -> u16 {
        self.filtered = match self.filtered {
            0 => centivolts << 4,
            filtered => filtered - (filtered >> 3) + (centivolts << 1),
        };
        self.filtered >> 4
    }
}

#[cfg(test)]
mod tests {
    extern crate std;

    use super::*;
    use std::vec::Vec;

    /// The conversion from first principles, in floating point.
    fn exact(reading: u16) -> f64 {
        let pin = 1.1 * f64::from(reading) / (4.0 * 1024.0);
        pin * 4.3 * 1024.0 / f64::from(SLOPE) * 100.0
    }

    #[test]
    fn matches_the_formula_for_every_reading_the_adc_can_give() {
        for reading in 0..=MAX_READING {
            let error = (f64::from(centivolts(reading)) - exact(reading)).abs();
            // Half a centivolt of rounding, plus what the constants lose.
            assert!(error < 0.6, "{reading} is off by {error}");
        }
    }

    #[test]
    fn rises_with_the_reading() {
        for reading in 0..MAX_READING {
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
        // Four saturated conversions are as high as this board can see.
        assert_eq!(centivolts(MAX_READING), 442);
    }

    #[test]
    fn a_reading_the_adc_cant_give_is_safe_rather_than_an_overflow() {
        assert_eq!(centivolts(MAX_READING + 1), u16::MAX);
        assert_eq!(centivolts(u16::MAX), u16::MAX);
    }

    #[test]
    fn samples_come_four_times_a_second() {
        assert_eq!(SAMPLES_PER_SECOND, 4);
    }

    /// Samples in `seconds` of real time.
    const fn secs(seconds: u16) -> u16 {
        seconds * SAMPLES_PER_SECOND
    }

    /// Feeds `samples` samples at a steady voltage and level, and returns
    /// every verdict.
    fn run(lvp: &mut Protection, cv: u16, level: u8, samples: u16) -> Vec<Verdict> {
        (0..samples).map(|_| lvp.sample(cv, level)).collect()
    }

    fn last(lvp: &mut Protection, cv: u16, level: u8, samples: u16) -> Verdict {
        *run(lvp, cv, level, samples).last().unwrap()
    }

    fn all(verdicts: &[Verdict], expected: Verdict) -> bool {
        verdicts.iter().all(|&v| v == expected)
    }

    /// The same as [`run`], with the light following the ceiling down from
    /// `level` the moment it moves.
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
        assert_eq!(last(&mut lvp, 295, 100, secs(5)), Verdict::Off);
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
        assert_eq!(after, [Verdict::Off]);
    }

    #[test]
    fn a_flat_cell_goes_down_the_ramp_in_big_steps_before_turning_off() {
        let mut lvp = Protection::new();
        let mut verdicts = follow(&mut lvp, 280, CEILING, secs(10));
        verdicts.dedup();
        let off = verdicts.iter().position(|&v| v == Verdict::Off).unwrap();
        assert_eq!(
            verdicts[..=off],
            [
                NONE,
                Verdict::Ceiling(90),
                Verdict::Ceiling(60),
                Verdict::Ceiling(30),
                Verdict::Ceiling(1),
                Verdict::Off
            ]
        );
    }

    #[test]
    fn a_cell_that_recovers_under_the_lighter_load_stays_on() {
        let mut lvp = Protection::new();
        assert_eq!(last(&mut lvp, 280, CEILING, secs(1)), Verdict::Ceiling(90));
        let held = Verdict::Ceiling(90);
        assert!(all(&run(&mut lvp, 315, 90, secs(60)), held));
    }

    #[test]
    fn waits_for_the_light_to_reach_the_ceiling_before_judging_it() {
        let mut lvp = Protection::new();
        assert_eq!(last(&mut lvp, 250, CEILING, secs(1)), Verdict::Ceiling(90));
        // Still fading down to it. The fade's over in half a second; this is
        // four, short of the slower hold.
        let held = Verdict::Ceiling(90);
        assert!(all(&run(&mut lvp, 250, 91, secs(4)), held));
        // There, a second of flat samples drops it again.
        let verdicts = run(&mut lvp, 250, 90, secs(1));
        let (before, after) = verdicts.split_at(usize::from(secs(1)) - 1);
        assert!(all(before, held));
        assert_eq!(after, [Verdict::Ceiling(60)]);
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
        let verdicts = run(&mut lvp, 250, 50, secs(10));
        // The filtered voltage passes 2.9 V a couple of seconds in, then the
        // hold has to run out before the ceiling moves.
        let dropped = verdicts.iter().position(|&v| v != NONE).unwrap();
        let dropped = u16::try_from(dropped).unwrap();
        assert!(dropped > secs(2) && dropped < secs(3), "{dropped}");
    }

    #[test]
    fn a_voltage_back_above_3_volts_restarts_the_hold() {
        let mut lvp = Protection::new();
        run(&mut lvp, 295, 100, secs(3));
        // Enough good samples to lift the filtered value over 3 V.
        run(&mut lvp, 310, 100, secs(2));
        // Without the restart this would be well past the hold.
        assert!(all(&run(&mut lvp, 295, 100, secs(5) - 1), NONE));
        assert!(run(&mut lvp, 295, 100, secs(5)).contains(&Verdict::Ceiling(85)));
    }

    #[test]
    fn does_nothing_while_the_torch_is_off() {
        let mut lvp = Protection::new();
        assert!(all(&run(&mut lvp, 250, 0, secs(600)), NONE));
        // And the hold starts from scratch when it comes on.
        assert_eq!(last(&mut lvp, 250, 1, secs(1) - 1), NONE);
        assert_eq!(lvp.sample(250, 1), Verdict::Off);
    }

    #[test]
    fn turning_off_keeps_the_ceiling() {
        let mut lvp = Protection::new();
        assert_eq!(last(&mut lvp, 295, 16, secs(5)), Verdict::Ceiling(1));
        assert!(run(&mut lvp, 250, 1, secs(5)).contains(&Verdict::Off));
        assert_eq!(last(&mut lvp, 280, 0, secs(1)), Verdict::Ceiling(1));
    }

    #[test]
    fn a_long_rest_keeps_the_ceiling_and_forgets_the_filter() {
        let mut lvp = Protection::new();
        assert_eq!(last(&mut lvp, 295, 60, secs(5)), Verdict::Ceiling(45));
        for _ in 0..FORGET_TICKS {
            lvp.rest();
        }
        assert_eq!(lvp.centivolts(), None);
        // The next sample stands alone, rather than filtering into 2.95 V.
        assert_eq!(lvp.sample(380, 0), Verdict::Ceiling(45));
        assert_eq!(lvp.centivolts(), Some(380));
    }

    #[test]
    fn a_short_rest_keeps_the_filter() {
        let mut lvp = Protection::new();
        run(&mut lvp, 280, 50, secs(1) - 1);
        for _ in 0..FORGET_TICKS - 1 {
            lvp.rest();
        }
        assert_eq!(lvp.centivolts(), Some(280));
    }

    #[test]
    fn a_sample_starts_the_wait_to_forget_again() {
        let mut lvp = Protection::new();
        for _ in 0..3 {
            for _ in 0..FORGET_TICKS - 1 {
                lvp.rest();
            }
            lvp.sample(380, 0);
        }
        assert_eq!(lvp.centivolts(), Some(380));
    }

    #[test]
    fn a_rest_starts_the_hold_afresh() {
        let mut lvp = Protection::new();
        run(&mut lvp, 250, 1, secs(1) - 1);
        lvp.rest();
        assert_eq!(last(&mut lvp, 250, 1, secs(1) - 1), NONE);
        assert_eq!(lvp.sample(250, 1), Verdict::Off);
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
        // The filter needs a couple of seconds to climb from 2.95 to 3.3 V.
        let verdicts = run(&mut lvp, 340, 35, secs(8));
        let up = verdicts
            .iter()
            .position(|&v| v == Verdict::Ceiling(50))
            .unwrap();
        let up = u16::try_from(up).unwrap();
        assert!(up >= secs(4) && up < secs(8), "{up}");
        assert_eq!(last(&mut lvp, 340, 0, secs(4)), Verdict::Ceiling(65));
        // Back to no limit, and no further.
        assert_eq!(last(&mut lvp, 340, 0, secs(120)), NONE);
    }

    #[test]
    fn the_filter_has_room_for_the_most_plausible_voltage() {
        let mut lvp = Protection::new();
        assert!(u32::from(*PLAUSIBLE.end()) << 4 <= u32::from(u16::MAX));
        assert_eq!(last(&mut lvp, *PLAUSIBLE.end(), CEILING, secs(60)), NONE);
        assert_eq!(lvp.centivolts(), Some(*PLAUSIBLE.end()));
    }

    #[test]
    fn plausible_covers_a_real_cell_and_nothing_broken() {
        for cv in [100, 250, 290, 310, 420, 435] {
            assert!(plausible(cv), "{cv}");
        }
        // A zero reading, four saturated conversions, one the ADC can't
        // give, and either side.
        for cv in [
            centivolts(0),
            centivolts(MAX_READING),
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
        assert!(verdicts.contains(&Verdict::Off));
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
    fn a_second_of_garbage_drops_to_the_fault_ceiling_and_never_off() {
        for level in [0, 30, FAULT_CEILING, CEILING] {
            let mut lvp = Protection::new();
            let verdicts = run(&mut lvp, u16::MAX, level, u16::from(GLITCHES));
            assert!(all(&verdicts, NONE), "level {level}");
            let fault = Verdict::Ceiling(FAULT_CEILING);
            assert_eq!(lvp.sample(u16::MAX, level), fault);
            assert!(all(&run(&mut lvp, 0, level, secs(600)), fault));
        }
    }

    #[test]
    fn garbage_keeps_a_lower_ceiling() {
        let mut lvp = Protection::new();
        assert_eq!(last(&mut lvp, 295, 40, secs(5)), Verdict::Ceiling(25));
        assert_eq!(last(&mut lvp, 0, 25, secs(10)), Verdict::Ceiling(25));
    }

    #[test]
    fn good_samples_bring_it_back_up_after_garbage() {
        let mut lvp = Protection::new();
        run(&mut lvp, 0, CEILING, secs(2));
        let verdicts = run(&mut lvp, 400, CEILING, secs(30));
        assert!(verdicts.contains(&Verdict::Ceiling(FAULT_CEILING + STEP)));
        assert_eq!(verdicts.last(), Some(&NONE));
    }

    #[test]
    fn garbage_never_counts_towards_stepping_up() {
        let mut lvp = Protection::new();
        assert_eq!(last(&mut lvp, 295, 50, secs(5)), Verdict::Ceiling(35));
        run(&mut lvp, 340, 35, secs(3));
        // Every third sample bad: never four good seconds in a row.
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
        // Every other sample bad: the flat ones still add up.
        for _ in 0..secs(2) {
            verdicts.push(lvp.sample(280, 1));
            verdicts.push(lvp.sample(u16::MAX, 1));
        }
        assert!(verdicts.contains(&Verdict::Off));
    }
}
