//! The cell's voltage, and what to do as it runs flat.
//!
//! The MCU runs from a regulator, so its own supply says nothing about the
//! cell. The cell comes in on PA1 (AIN1) through a divider,
//! and the ADC reads that against the internal 1.1 V reference, four
//! conversions accumulated.
//!
//! ```text
//! pin  = 1.1 V × reading / (4 × 1024)
//! cell = pin × 4.3 × 1024 / SLOPE
//! ```
//!
//! which in centivolts comes to `reading × 473 / (4 × SLOPE)`. Those are the
//! facts of the stock firmware's voltage sums and its slope for this board.
//! The effective ratio is 4.3 × 1024 / 1094, about 4.03, so the ADC runs out
//! at about 4.42 V (docs/hardware.md).
//!
//! `reading × 473` needs 21 bits, and there's no runtime 32-bit multiply on
//! this chip (docs/toolchain.md). So this splits the reading into its high
//! and low bytes, each multiplied by a constant worked out at compile time
//! that's scaled to keep the product in a `u16`.
//!
//! ```text
//! centivolts = (hi × HIGH + ((lo × LOW) >> 4) + 64) >> 7
//! ```
//!
//! That's in 128ths of a centivolt until the last shift. The two rounded
//! constants cost under a tenth of a centivolt, which next to a calibration
//! from one sample is nothing.
//!
//! The thresholds are this board's. The Wurkkos 21700 is
//! unprotected, and the MP3432 keeps running down to 0.8 V on its input once
//! it's started, pulling more current as the voltage falls, so nothing but
//! this module stops the cell being run into the ground.

use crate::TICKS_PER_SECOND;
use crate::gradual::Ceiling;
use crate::ramp::CEILING;
use crate::sensors::CYCLE_TICKS;

/// The stock firmware's voltage slope for this board.
///
/// It's against its default of 1024, higher reading lower. Calibrated on one
/// of ToyKeeper's samples, and unmeasured on any light of ours. Its offset is
/// 0 here, so there's no offset to carry.
pub const SLOPE: u16 = 1094;

/// 1.1 V in centivolts times the 4.3 that a slope of 1024 stands for.
const NUMERATOR: u32 = 473;

/// Four full-scale 10-bit conversions, the most the ADC can give.
const MAX_READING: u16 = 4 * 1023;

/// `NUMERATOR / (4 × SLOPE)` centivolts a count, times `scale`, rounded.
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

/// 256 counts in 128ths of a centivolt.
const HIGH: u16 = scaled(256 << 7);

/// One count in 2048ths of a centivolt, sixteen times finer than [`HIGH`]
/// because it's a small number that needs the digits.
const LOW: u16 = scaled(1 << 11);

#[expect(
    clippy::as_conversions,
    reason = "widening to u32 in a const, where From can't be called"
)]
const _: () = assert!(LOW as u32 * 0xFF <= u16::MAX as u32);
#[expect(
    clippy::as_conversions,
    reason = "widening to u32 in a const, where From can't be called"
)]
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
#[expect(
    clippy::arithmetic_side_effects,
    reason = "reading is at most MAX_READING here, and the asserts on HIGH and LOW bound \
              every step for that"
)]
pub const fn centivolts(reading: u16) -> u16 {
    if reading > MAX_READING {
        return u16::MAX;
    }
    let (hi, lo) = (reading >> 8, reading & 0xFF);
    (hi * HIGH + ((lo * LOW) >> 4) + 64) >> 7
}

/// Four voltage samples a second, one every [`CYCLE_TICKS`].
#[expect(
    clippy::as_conversions,
    reason = "widening u8 to u16 in a const, where From can't be called"
)]
pub const SAMPLES_PER_SECOND: u16 = TICKS_PER_SECOND / CYCLE_TICKS as u16;

#[expect(
    clippy::as_conversions,
    reason = "widening u8 to u16 in a const, where From can't be called"
)]
const _: () = assert!(TICKS_PER_SECOND.is_multiple_of(CYCLE_TICKS as u16));

/// Below this many centivolts, held for [`STEP_SAMPLES`], the ceiling steps
/// down. It's a tenth of a volt over [`OFF_BELOW`], so the light starts
/// dimming before the cell gets there. Unmeasured.
pub const STEP_DOWN_BELOW: u16 = 300;

/// Below this many centivolts, held for [`HOLD_SAMPLES`], the ceiling drops
/// [`FLAT_STEP`] levels, and at level 1 the torch turns off.
///
/// It's the stock firmware's default low-voltage threshold, which the TS26's
/// build doesn't override. Wurkkos quotes 2.8 V, but with an unprotected cell the
/// higher figure is the one to have. It also leaves 0.2 V, about 7%, over the
/// MP3432's 2.7 V start-up, so a light that's cut out can start again, and
/// that 7% has to cover the 1.1 V reference's tolerance and a divider ratio
/// nobody's checked. If the ratio's really the 4.3 of the old comment, this
/// reads low and cuts out early, which is the safe way to be wrong.
/// Unmeasured.
pub const OFF_BELOW: u16 = 290;

/// At or above this many centivolts, held for [`RECOVER_SAMPLES`], the
/// ceiling steps back up.
///
/// The 0.3 V gap to [`STEP_DOWN_BELOW`] is the hysteresis, so a cell that
/// sags to 2.95 V at a high level and rests at 3.2 V on a lower one stays
/// put. Unmeasured.
pub const RECOVER_AT: u16 = 330;

/// At or above this many centivolts, in one sample with the light off, a
/// flat cell counts as charged ([`Protection::flat`]).
///
/// It's one sample because a press only gets one before its click counts.
/// So it's set well above where a flat cell rests, since a rested Li-ion at
/// 3.3 V is close to empty, and 3.6 V means it's really taken some charge.
/// A reading off by a few tenths of a volt in a sum of four conversions would
/// be a broken ADC rather than noise. Unmeasured.
pub const CHARGED_AT: u16 = 360;

/// The MP3432's start-up voltage from its datasheet, in centivolts.
const BOOST_START: u16 = 270;

const _: () = assert!(BOOST_START < OFF_BELOW);
const _: () = assert!(OFF_BELOW < STEP_DOWN_BELOW && STEP_DOWN_BELOW < RECOVER_AT);
const _: () = assert!(RECOVER_AT < CHARGED_AT && CHARGED_AT < *PLAUSIBLE.end());

/// How long the filtered voltage has to stay under [`OFF_BELOW`] before it
/// acts. One second, and unmeasured.
pub const HOLD_SAMPLES: u16 = SAMPLES_PER_SECOND;

/// How long it has to stay under [`STEP_DOWN_BELOW`] for each step down, five
/// seconds.
///
/// Long enough for the cell to recover a little after the last step, the way
/// the stock firmware waits about six seconds between its low-voltage
/// step-downs. Unmeasured.
pub const STEP_SAMPLES: u16 = SAMPLES_PER_SECOND * 5;

/// How long it has to stay at [`RECOVER_AT`] or above for each step back up,
/// four seconds. Unmeasured.
pub const RECOVER_SAMPLES: u16 = SAMPLES_PER_SECOND * 4;

/// How many levels each step down (or up) moves the ceiling, a tenth of the
/// ramp. Unmeasured.
pub const STEP: u8 = 15;

/// How many levels the ceiling drops for each hold under [`OFF_BELOW`], a
/// fifth of the ramp. From the top of the ramp that's four drops and then
/// off, around seven seconds with the fades. Unmeasured.
pub const FLAT_STEP: u8 = 30;

/// Ticks of rest before the filter forgets the cell, five minutes.
///
/// Any sooner and clicking off and on would restart it from the unloaded
/// voltage each time, which keeps a nearly flat cell running. Any later and a
/// cell that's sat for days gets judged on what it read before. Unmeasured.
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

/// [`Protection`]'s filter holds 16 times a plausible sample and adds twice
/// another before it takes an eighth off, so 18 times the top has to fit.
#[expect(
    clippy::as_conversions,
    reason = "widening to u32 in a const, where From can't be called"
)]
const _: () = assert!(*PLAUSIBLE.end() as u32 * 18 <= u16::MAX as u32);

/// Implausible samples in a row that [`Protection`] lets go before it gives
/// up on the cell. The fourth in a row, a second's worth, is
/// [`Verdict::Blind`]. Unmeasured.
pub const GLITCHES: u8 = 3;

/// Ticks per level as the ceiling fades down, and back up. Down is a level a
/// tick, quicker than thermal's, since the cell's already under its threshold
/// by the time it steps. A drop of 15 levels takes a quarter of a second, and
/// a drop of 30 under half. Unmeasured.
const DOWN_EVERY: u8 = 1;
const UP_EVERY: u8 = 4;

/// The low-voltage verdict for one sample.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// The ceiling's heading for this level, [`CEILING`] meaning no limit.
    /// [`Protection::ceiling`] is where it's got to.
    Ceiling(u8),
    /// The cell's flat. Turn off now, and stay off until a reading with the
    /// light off says it's been charged ([`Protection::flat`]).
    Flat,
    /// More than [`GLITCHES`] samples in a row that can't be real. With no
    /// reading there's no low-voltage cut-off, so the caller treats this as a
    /// fault.
    Blind,
}

/// Low-voltage protection.
///
/// Feed it each voltage sample (every [`CYCLE_TICKS`] ticks), call
/// [`Protection::tick`] every tick, and [`Protection::ceiling`] says how
/// bright the torch may be. The ceiling fades rather than jumps, like
/// thermal's.
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
/// Turning off for a flat cell latches [`Protection::flat`]. The light stays
/// off until a sample with it off, which a press of the switch takes before
/// the click counts, reads [`CHARGED_AT`] or more. That's a cell that's been
/// charged or swapped, since one that cut out at level 1 rests only a little
/// above where it cut. Without the latch, charging in place would leave the
/// torch dead until the cell came out, and without the threshold every click
/// would run a flat cell at level 1 again. That sample seeds the filter
/// afresh and takes the ceiling off, so the light comes back as if new, and
/// a cell that's still weak goes through the steps above again. Unmeasured.
///
/// A sample that isn't [`plausible`] never reaches the filter. It can't
/// count towards stepping back up, and it doesn't restart the holds, so a
/// flat cell can't hide behind the odd bad reading. More than [`GLITCHES`]
/// in a row is [`Verdict::Blind`], on or off, since with no reading there's
/// no low-voltage cut-off and this cell has no protection of its own.
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
    /// Turned off for a flat cell, and not yet read as charged.
    flat: bool,
    ceiling: Ceiling,
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
            flat: false,
            ceiling: Ceiling::new(DOWN_EVERY, UP_EVERY),
            rested: 0,
        }
    }

    /// The ceiling right now.
    pub const fn ceiling(&self) -> u8 {
        self.ceiling.level()
    }

    /// Whether it's turned off for a flat cell and nothing's read the cell
    /// as charged since.
    pub const fn flat(&self) -> bool {
        self.flat
    }

    /// Sends the ceiling down to level 1, fading from `level`, for a caller
    /// that's giving up on the cell.
    pub fn fail(&mut self, level: u8) {
        self.ceiling.set(1, level);
    }

    /// One tick. Moves the ceiling a step towards where it's heading and
    /// returns it.
    pub const fn tick(&mut self) -> u8 {
        self.ceiling.tick()
    }

    /// One tick with nothing measuring the cell. The holds start again, as
    /// they do for any time off, and so does the count of implausible
    /// samples. The ceiling stays, since only good readings raise it, and so
    /// does [`Protection::flat`]. The filter stays too, until
    /// [`FORGET_TICKS`] have gone by.
    pub const fn rest(&mut self) {
        (self.low_samples, self.flat_samples, self.high_samples) = (0, 0, 0);
        self.implausible = 0;
        self.rested = self.rested.saturating_add(1);
        if self.rested >= FORGET_TICKS {
            self.filtered = 0;
        }
    }

    /// The filtered voltage in centivolts, or `None` before the first sample.
    pub const fn centivolts(&self) -> Option<u16> {
        match self.filtered {
            0 => None,
            filtered => Some(filtered >> 4),
        }
    }

    /// One voltage sample, with `centivolts` from [`centivolts`] and the level
    /// the torch is at (`0` for off).
    pub fn sample(&mut self, centivolts: u16, level: u8) -> Verdict {
        let verdict = self.judge(centivolts, level);
        if let Verdict::Ceiling(target) = verdict {
            self.ceiling.set(target, level);
        }
        verdict
    }

    /// The verdict on a sample, and where the ceiling should head for, which
    /// [`Protection::sample`] sends it to.
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
            // Charged, or a new cell. Judge it from here, not from the
            // filter's memory of it under load.
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
            return Verdict::Ceiling(ceiling);
        }
        self.low_samples = match voltage < STEP_DOWN_BELOW {
            true => self.low_samples.saturating_add(1),
            false => 0,
        };
        self.flat_samples = match voltage < OFF_BELOW {
            // Still fading down to the ceiling, so the cell's still under
            // the heavier load. Wait till it's there and judge it on that.
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
        // Only a cell under 3 V gets here, so it can't have recovered.
        let from = ceiling.min(level);
        if from == 1 {
            self.flat = true;
            return Verdict::Flat;
        }
        Verdict::Ceiling(from.saturating_sub(drop).max(1))
    }

    /// Folds a sample into the moving average and returns the result in
    /// centivolts. In sixteenths, moving an eighth of the way is taking off
    /// an eighth and adding the sample times two. Only plausible samples get
    /// here, so the sum stays well inside a `u16` and the seed isn't zero.
    ///
    /// With every sample plausible, `filtered` never passes 16 times the top
    /// of [`PLAUSIBLE`], so the sum stays under 18 times it, which the assert
    /// after [`plausible`] checks. Release builds, which check for overflow,
    /// can't see that, so the add is checked. Were it ever to overflow, this
    /// returns 0 once and [`Protection::centivolts`] reads `None`. That one
    /// low sample can't step anything down, since `flat_samples` needs
    /// [`HOLD_SAMPLES`] in a row, and the next sample takes the `0 =>` arm and
    /// seeds the filter afresh from that raw sample, unsmoothed. The assert
    /// above makes that unreachable. It's a fallback rather than a saturating
    /// add because saturating would read as a high voltage and hold protection
    /// off.
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
    fn a_flat_cell_goes_down_the_ramp_in_big_steps_before_turning_off() {
        let mut lvp = Protection::new();
        let mut verdicts = follow(&mut lvp, 280, CEILING, secs(10));
        verdicts.dedup();
        let off = verdicts.iter().position(|&v| v == Verdict::Flat).unwrap();
        assert_eq!(
            verdicts[..=off],
            [
                NONE,
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
        assert_eq!(lvp.sample(250, 1), Verdict::Flat);
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
            lvp.rest();
        }
        assert_eq!(lvp.centivolts(), None);
        // The next sample stands alone, rather than filtering into 2.95 V.
        assert_eq!(lvp.sample(380, 0), Verdict::Ceiling(45));
        assert_eq!(lvp.centivolts(), Some(380));
    }

    #[test]
    fn the_filter_forgets_the_cell_after_five_minutes_of_rest() {
        // Five minutes of ticks at 64 Hz.
        assert_eq!(TICKS_PER_SECOND, 64);
        assert_eq!(FORGET_TICKS, 19_200);
        assert_eq!(FORGET_TICKS / TICKS_PER_SECOND, 5 * 60);
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
        assert_eq!(lvp.sample(250, 1), Verdict::Flat);
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
        assert!(u16::try_from(u32::from(*PLAUSIBLE.end()) << 4).is_ok());
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
            // A dead ADC, either side of plausible, four saturated
            // conversions, and a reading the ADC can't give.
            for garbage in [0, 99, 441, centivolts(MAX_READING), u16::MAX] {
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
        // Any reading it can believe, good news or not.
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
        lvp.rest();
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

    /// A cell run flat at level 1, the only level it turns off from.
    fn flat() -> Protection {
        let mut lvp = Protection::new();
        assert_eq!(last(&mut lvp, 250, 1, secs(1)), Verdict::Flat);
        assert!(lvp.flat());
        lvp
    }

    #[test]
    fn flat_stays_flat_until_a_reading_with_the_light_off_says_its_charged() {
        let mut lvp = flat();
        let quiet =
            |verdicts: Vec<Verdict>| verdicts.iter().all(|v| matches!(v, Verdict::Ceiling(_)));
        // Rested, even well enough to lift a ceiling, but not charged.
        assert!(quiet(run(&mut lvp, CHARGED_AT - 1, 0, secs(600))));
        assert!(lvp.flat());
        // Only a reading with the light off counts.
        assert!(quiet(run(&mut lvp, 400, 1, secs(10))));
        assert!(lvp.flat());
        assert_eq!(lvp.sample(CHARGED_AT, 0), NONE);
        assert!(!lvp.flat());
        // As good as new, judged from that reading alone.
        assert_eq!(lvp.ceiling(), CEILING);
        assert_eq!(lvp.centivolts(), Some(CHARGED_AT));
    }

    #[test]
    fn a_rest_keeps_it_flat() {
        let mut lvp = flat();
        for _ in 0..FORGET_TICKS {
            lvp.rest();
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
        // Every third sample bad, so never four good seconds in a row.
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
        // Every other sample bad, but the flat ones still add up.
        for _ in 0..secs(2) {
            verdicts.push(lvp.sample(280, 1));
            verdicts.push(lvp.sample(u16::MAX, 1));
        }
        assert!(verdicts.contains(&Verdict::Flat));
    }

    /// Where a voltage held for ever should end up, worked out from the public
    /// thresholds alone and not by asking [`Protection`].
    #[derive(Debug, PartialEq)]
    enum Held {
        /// A cell that's fine, so no limit at all.
        Fine,
        /// A cell that's plausible and flat, so it ends up off.
        Flat,
        /// A reading that can't be real, so blind once [`GLITCHES`] have gone.
        Unreadable,
    }

    fn held(cv: u16) -> Held {
        if !PLAUSIBLE.contains(&cv) {
            Held::Unreadable
        } else if cv < STEP_DOWN_BELOW {
            Held::Flat
        } else {
            Held::Fine
        }
    }

    #[test]
    fn every_reading_held_ends_where_the_thresholds_say() {
        let mut ends = [0_u32; 3];
        for raw in 0..=MAX_READING {
            let cv = centivolts(raw);
            let expected = held(cv);
            let mut lvp = Protection::new();
            // The UI is at the top and the light follows the ceiling down,
            // the way `limits` runs it.
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
                    // Several holds, one for each step down the ramp.
                    assert!(at >= HOLD_SAMPLES, "raw {raw} ({cv} cV) off at {at}");
                }
                Held::Unreadable => {
                    let blind = u16::from(GLITCHES) + 1;
                    assert_eq!(off_at, Some(blind), "raw {raw} ({cv} cV)");
                }
            }
            ends[match expected {
                Held::Fine => 0,
                Held::Flat => 1,
                Held::Unreadable => 2,
            }] += 1;
        }
        // Every kind turns up, or the classifier isn't being tested.
        assert!(ends.iter().all(|&count| count > 0), "{ends:?}");
    }
}
