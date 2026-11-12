//! Thermal regulation, with temperature readings in and a ceiling on the level
//! out.
//!
//! Feed it a reading from `temp::sixteenths` every [`CYCLE_TICKS`] ticks with
//! [`Thermal::sample`], call [`Thermal::tick`] every tick, and run the light
//! at whichever is lower, the UI's level or the ceiling. The ceiling's a
//! [`Ceiling`], so a step-down is a fade of at most one level every other
//! tick, never a jump.
//!
//! The approach follows the stock firmware's (facts only, it's GPL-3.0).
//!
//! - Smooth the readings, then predict where the temperature is heading from
//!   how far it's moved over the last eight samples, times a lookahead of
//!   four. The stock firmware uses the same history length and lookahead.
//! - If the prediction is over the limit and it isn't already cooling, step
//!   down. The error adds up over readings and each step spends some of it,
//!   so a little over steps down now and then and a lot over steps down
//!   every reading. The stock firmware's warning threshold works much the
//!   same way.
//! - If the prediction is more than 2 °C under the limit (the stock
//!   firmware's window) and it isn't warming up, step back up the same way,
//!   but slower and never more than a level a reading. In between, hold. That
//!   gap, the slow climb and the lookahead are what stop it hunting.
//! - Never step down below a floor. The stock firmware's TS26 build stops at
//!   level 50 of 150, so this does too, though the same number on this
//!   firmware's own curve (`src/ramp.rs`) needn't be the same brightness.
//!
//! Only sample while the light's on. The stock firmware leaves the
//! temperature alone in standby because the ADC costs about 250 µA against
//! about 20 µA asleep, and the torch will sleep when off. Call
//! [`Thermal::reset`] when it turns on, so the readings and ceiling from last
//! time don't carry over. A light that turns on hot then steps down from the
//! first readings.
//! - Ignore a reading that's impossible or far from the last one, unless a
//!   few in a row agree. If the sensor keeps reading impossible values, step
//!   down to the floor, since a broken sensor can't say it's cool.
//!
//! The numbers here are a first guess, carried over from an earlier torch
//! where they aren't the stock firmware's. None of them has been measured on a
//! TS26, so expect them to change once one's been run up to temperature.
//! That's why `ramp::CEILING` holds the light back in the meantime.

use crate::gradual::Ceiling;
use crate::ramp::CEILING;
use crate::sensors::CYCLE_TICKS;

/// Kelvin, from whole degrees Celsius.
#[expect(
    clippy::as_conversions,
    reason = "widening u8 to u16 in a const fn, where From can't be called"
)]
pub const fn celsius(degrees: u8) -> u16 {
    degrees as u16 + 273
}

/// The temperature to hold the MCU at, 50 °C, in kelvin.
///
/// The stock firmware's figure for Wurkkos lights. Unmeasured here. 1Lumen
/// saw 52 °C on the head on one sample, and nobody knows what the MCU reads
/// when the head's at that.
pub const LIMIT: u16 = celsius(50);

/// The lowest level thermal regulation steps down to.
///
/// The stock firmware's level number, on this firmware's curve. Unmeasured.
/// Nobody knows that this level is cool enough to hold in still air, or what
/// it draws.
pub const FLOOR: u8 = 50;

const _: () = assert!(FLOOR <= CEILING);

/// Readings and the smoothed temperature are in sixteenths of a kelvin, which
/// is what `temp::sixteenths` hands over.
const FRACTION: i16 = 16;
/// Samples of history to measure the trend over. The stock firmware's figure,
/// unmeasured on this light.
const HISTORY: usize = 8;
/// How many trend-lengths ahead to predict. The stock firmware's figure,
/// unmeasured on this light.
const LOOKAHEAD: i16 = 4;
/// How far under the limit the prediction has to be before stepping up.
/// The stock firmware's figure, unmeasured on this light.
const WINDOW: i16 = 2 * FRACTION;
/// A trend at least this steep counts as warming up, and blocks stepping up.
/// Unmeasured.
const WARMING: i16 = FRACTION;
/// The steepest trend the prediction believes, 64 °C over the history's two
/// seconds. Unmeasured.
const MAX_TREND: i16 = 64 * FRACTION;
/// How much accumulated error, in sixteenths of a kelvin over the limit
/// summed over readings, buys one level down. Unmeasured.
const DOWN_PRESSURE: i16 = 4 * FRACTION;
/// The same for one level up, counted from the bottom of the window.
/// Unmeasured.
const UP_PRESSURE: i16 = 16 * FRACTION;
/// The most levels to step down per sample. With a sample every 16 ticks
/// and the ceiling moving a level every other tick, that's as fast as the
/// ceiling can follow. Unmeasured.
const MAX_STEP_DOWN: u8 = 8;
/// Ticks per level as the ceiling fades down, and up. Unmeasured.
const DOWN_EVERY: u8 = 2;
const UP_EVERY: u8 = 4;
/// Readings outside -40 °C to 150 °C can't be real. The part is rated to
/// 125 °C at most, so the top leaves some slack. In kelvin. Unmeasured.
const PLAUSIBLE: core::ops::RangeInclusive<u16> = 233..=423;

/// [`LIMIT`] in sixteenths, which fits an `i16` because it's plausible.
const LIMIT_SIXTEENTHS: i16 = LIMIT.cast_signed() * FRACTION;

const _: () = assert!(LIMIT <= *PLAUSIBLE.end());

/// Whether a reading in sixteenths of a kelvin could be real. Anything else
/// is a broken sensor or a bad ADC setup.
pub fn plausible(sixteenths: u16) -> bool {
    PLAUSIBLE_SIXTEENTHS.contains(&sixteenths)
}

/// [`PLAUSIBLE`] in sixteenths, worked out at compile time.
const PLAUSIBLE_SIXTEENTHS: core::ops::RangeInclusive<u16> =
    *PLAUSIBLE.start() * FRACTION.cast_unsigned()..=*PLAUSIBLE.end() * FRACTION.cast_unsigned();

/// [`PLAUSIBLE_SIXTEENTHS`] as the smoothed temperature's `i16`. The smoothed
/// temperature and its history never leave it, and clamping to it where
/// they're read shows release builds, which check for overflow, that the
/// sums on them can't wrap. The clamps never change a value.
const NOW_LOW: i16 = PLAUSIBLE_SIXTEENTHS.start().cast_signed();
const NOW_HIGH: i16 = PLAUSIBLE_SIXTEENTHS.end().cast_signed();

/// The prediction is a plausible reading plus [`LOOKAHEAD`] times a trend
/// clamped to [`MAX_TREND`], less [`LIMIT_SIXTEENTHS`], and every step of that
/// fits an `i16` either way.
#[expect(
    clippy::as_conversions,
    reason = "widening to i32 in a const, where From can't be called"
)]
const _: () = {
    let (low, high) = (
        *PLAUSIBLE.start() as i32 * FRACTION as i32,
        *PLAUSIBLE.end() as i32 * FRACTION as i32,
    );
    let swing = MAX_TREND as i32 * LOOKAHEAD as i32;
    assert!(high + swing <= i16::MAX as i32);
    assert!(low - swing - LIMIT_SIXTEENTHS as i32 >= i16::MIN as i32);
};

/// A reading further than this from the last good one, in sixteenths of a
/// kelvin, is a glitch, unless more than `GLITCHES` in a row agree, and then
/// it's believed. Unmeasured.
const JUMP: u16 = 6 * FRACTION.cast_unsigned();
const GLITCHES: u8 = 3;

#[expect(
    clippy::as_conversions,
    reason = "widening u8 to u16 in a const, where From can't be called"
)]
const _: () = assert!(MAX_STEP_DOWN as u16 * DOWN_EVERY as u16 <= CYCLE_TICKS as u16);

/// For wrapping an index into the history, a power of two long.
#[expect(
    clippy::cast_possible_truncation,
    clippy::as_conversions,
    reason = "the assert below holds HISTORY to a u8, and try_from can't be called in a const"
)]
const HISTORY_MASK: u8 = HISTORY as u8 - 1;

#[expect(
    clippy::as_conversions,
    reason = "widening u8 to usize in a const, where From can't be called"
)]
const _: () = assert!(HISTORY.is_power_of_two() && HISTORY <= u8::MAX as usize);

/// The smoothed temperature, in sixteenths of a kelvin, and the last
/// [`HISTORY`] samples of it.
#[derive(Clone, Debug)]
struct Smoothed {
    now: i16,
    history: [i16; HISTORY],
    /// Where the oldest sample is in `history`.
    next: u8,
    /// The last reading that wasn't a glitch.
    last: u16,
}

impl Smoothed {
    /// Starts `slot` afresh from `reading`, as if it had always been there.
    /// Only for plausible readings, which keep it well inside an `i16`.
    ///
    /// The history's filled in once it's in place. Built whole, it goes
    /// through a copy on the stack, and that pulls in `memcpy`, which is
    /// bigger than the `memset` this costs.
    fn start(slot: &mut Option<Self>, reading: u16) -> &mut Self {
        let now = reading.cast_signed();
        let smoothed = slot.insert(Self {
            now,
            history: [0; HISTORY],
            next: 0,
            last: reading,
        });
        smoothed.history = [now; HISTORY];
        smoothed
    }

    /// An eighth of the way to `reading`, but always at least a sixteenth,
    /// so a steady reading is reached exactly.
    ///
    /// `now` starts at a plausible reading and only ever moves part of the
    /// way towards another, so it stays inside [`PLAUSIBLE_SIXTEENTHS`], and
    /// so does every sample in the history. Returns the new value.
    #[expect(
        clippy::arithmetic_side_effects,
        reason = "now is clamped to PLAUSIBLE_SIXTEENTHS and reading is plausible, so they're \
                  under 6,800 apart, and the step never overshoots reading"
    )]
    fn follow(&mut self, reading: u16) -> i16 {
        let now = self.now.clamp(NOW_LOW, NOW_HIGH);
        let difference = reading.cast_signed() - now;
        self.now = now
            + match difference / 8 {
                0 => difference.signum(),
                step => step,
            };
        self.now
    }

    /// Records `now`, the smoothed temperature from [`follow`](Self::follow)
    /// or a fresh start, and returns how far it's moved over the history.
    #[expect(
        clippy::indexing_slicing,
        reason = "masking with HISTORY_MASK keeps the index under HISTORY, a power of two"
    )]
    #[expect(
        clippy::arithmetic_side_effects,
        reason = "now is in PLAUSIBLE_SIXTEENTHS, as follow or a plausible reading left it, \
                  and the oldest sample is clamped to it"
    )]
    fn trend(&mut self, now: i16) -> i16 {
        let next = &mut self.history[usize::from(self.next & HISTORY_MASK)];
        let oldest = core::mem::replace(next, now);
        self.next = self.next.wrapping_add(1) & HISTORY_MASK;
        now - oldest.clamp(NOW_LOW, NOW_HIGH)
    }
}

#[derive(Clone, Debug)]
pub struct Thermal {
    ceiling: Ceiling,
    /// `None` before the first good reading. Only the tag needs writing to
    /// start afresh, so neither building a `Thermal` nor a reset clears the
    /// history.
    smoothed: Option<Smoothed>,
    /// Glitches in a row.
    rejected: u8,
    /// Error summed over readings. Positive pushes the ceiling down,
    /// negative pushes it up.
    pressure: i16,
}

impl Default for Thermal {
    fn default() -> Self {
        Self::new()
    }
}

impl Thermal {
    pub const fn new() -> Self {
        Self {
            ceiling: Ceiling::new(DOWN_EVERY, UP_EVERY),
            smoothed: None,
            rejected: 0,
            pressure: 0,
        }
    }

    /// The ceiling right now.
    pub const fn ceiling(&self) -> u8 {
        self.ceiling.level()
    }

    /// Forgets everything, so there's no ceiling, and the next reading starts
    /// the smoothing and the trend afresh. Call it when the light turns on,
    /// since nothing's sampled while it's off.
    pub const fn reset(&mut self) {
        *self = Self::new();
    }

    /// One tick. Moves the ceiling a step towards where it's heading and
    /// returns it.
    pub const fn tick(&mut self) -> u8 {
        self.ceiling.tick()
    }

    /// One temperature reading in sixteenths of a kelvin, taken every
    /// [`CYCLE_TICKS`] ticks while the light's on, and the level it's at
    /// right now.
    pub fn sample(&mut self, reading: u16, level: u8) {
        if !plausible(reading) {
            self.rejected = self.rejected.saturating_add(1);
            if self.rejected > GLITCHES {
                // The sensor's broken. Assume the worst.
                self.step_down(level, MAX_STEP_DOWN);
            }
            return;
        }
        // The smoothed temperature comes out of the match rather than being
        // read back, so release builds can see the range it's in.
        let (smoothed, smooth) = match &mut self.smoothed {
            Some(smoothed) if reading.abs_diff(smoothed.last) <= JUMP => {
                let smooth = smoothed.follow(reading);
                (smoothed, smooth)
            }
            Some(_) if self.rejected < GLITCHES => {
                self.rejected = self.rejected.saturating_add(1);
                return;
            }
            // The first reading, or a jump that's lasted long enough to be
            // real.
            slot => (Smoothed::start(slot, reading), reading.cast_signed()),
        };
        self.rejected = 0;
        smoothed.last = reading;
        let trend = smoothed.trend(smooth).clamp(-MAX_TREND, MAX_TREND);
        #[expect(
            clippy::arithmetic_side_effects,
            reason = "smooth is in PLAUSIBLE_SIXTEENTHS and trend is clamped, and the assert \
                      after PLAUSIBLE_SIXTEENTHS bounds both sums"
        )]
        let over = smooth + trend * LOOKAHEAD - LIMIT_SIXTEENTHS;
        if over > 0 && (trend >= 0 || smooth > LIMIT_SIXTEENTHS + WINDOW) {
            // Each step down costs `DOWN_PRESSURE`, so the rate follows how
            // far over the prediction is. A kelvin over steps once a second,
            // four over steps every reading, and a sixteenth over takes
            // sixteen seconds.
            let pressure = self.pressure.max(0).saturating_add(over);
            let steps = (pressure / DOWN_PRESSURE).min(i16::from(MAX_STEP_DOWN));
            self.pressure = pressure
                .saturating_sub(steps.saturating_mul(DOWN_PRESSURE))
                .min(DOWN_PRESSURE - 1);
            if steps > 0 {
                let steps = u8::try_from(steps).unwrap_or(MAX_STEP_DOWN);
                self.step_down(level, steps);
            }
        } else if over < -WINDOW && trend < WARMING {
            // The same going up, counted from the bottom of the window, but
            // at most a level a reading and for four times the pressure. The
            // head takes a minute or so to show what a step up did, so
            // coming back up too fast is what makes it hunt.
            let pressure = self
                .pressure
                .min(0)
                .saturating_add(over.saturating_add(WINDOW));
            if pressure <= -UP_PRESSURE {
                self.pressure = pressure.saturating_add(UP_PRESSURE).max(1 - UP_PRESSURE);
                self.ceiling.raise();
            } else {
                self.pressure = pressure;
            }
        } else if over <= 0 {
            // Inside the window, so hold and forget the push either way.
            self.pressure = 0;
        }
        // Over the limit but already cooling, so hold and keep the pressure
        // for if it turns.
    }

    /// Steps the ceiling down, never under [`FLOOR`], counting from the
    /// light's level if that's lower, so the first step shows straight away.
    fn step_down(&mut self, level: u8, steps: u8) {
        let from = match level {
            0 => self.ceiling.target(),
            level => self.ceiling.target().min(level.max(FLOOR)),
        };
        let target = from.saturating_sub(steps).max(FLOOR);
        self.ceiling.set(target, level);
    }
}

#[cfg(test)]
mod tests {
    // Only for `TRACE=1 cargo test -q thermal -- --nocapture`, which prints
    // what each closed-loop stretch did. Handy when retuning.
    extern crate std;
    use super::*;
    use crate::ramp::{self, DSM_TOP};

    /// The light at whatever level the UI asks for, capped by thermal
    /// regulation, checking every tick that the cap never moves the output
    /// more than one level.
    struct Run {
        thermal: Thermal,
        output: u8,
    }

    impl Run {
        fn new() -> Self {
            Self {
                thermal: Thermal::default(),
                output: 0,
            }
        }

        /// One reading in whole kelvin and the ticks up to the next. Returns
        /// the output.
        fn sample(&mut self, kelvin: u16, ui: u8) -> u8 {
            self.sample_fraction(fraction(kelvin), ui)
        }

        /// The same for a reading in sixteenths of a kelvin.
        fn sample_fraction(&mut self, reading: u16, ui: u8) -> u8 {
            self.thermal.sample(reading, self.output);
            for _ in 0..CYCLE_TICKS {
                self.tick(ui);
            }
            self.output
        }

        fn tick(&mut self, ui: u8) {
            let output = ui.min(self.thermal.tick());
            // Turning on is the UI's jump to make. Everything else is the
            // ceiling's doing, and that can't jump.
            if self.output != 0 {
                assert!(
                    output.abs_diff(self.output) <= 1,
                    "jumped {} → {output}",
                    self.output
                );
            }
            self.output = output;
        }

        fn hold(&mut self, kelvin: u16, ui: u8, samples: u16) -> u8 {
            for _ in 0..samples {
                self.sample(kelvin, ui);
            }
            self.output
        }
    }

    /// A made-up pseudo-random sequence, so the tests can add noise.
    struct Noise(u32);

    impl Noise {
        /// Something in `-spread..=spread`.
        fn next(&mut self, spread: i16) -> i16 {
            self.0 = self.0.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            ((self.0 >> 16) % (2 * spread as u32 + 1)) as i16 - spread
        }
    }

    fn noisy(kelvin: u16, noise: &mut Noise, spread: i16) -> u16 {
        kelvin.saturating_add_signed(noise.next(spread))
    }

    /// A whole number of kelvin as a reading, keeping junk well out of range.
    fn fraction(kelvin: u16) -> u16 {
        kelvin.saturating_mul(FRACTION as u16)
    }

    /// A rough head, with heat in from the LEDs, out to the air, and the MCU a
    /// few seconds behind the head. Nothing about it is measured; it's
    /// there to give the controller something with lag to fight against.
    /// Temperatures are in thousandths of a kelvin.
    struct Head {
        head: i32,
        mcu: i32,
        ambient: i32,
    }

    impl Head {
        /// About how far over ambient full power would take the head if
        /// nothing stopped it.
        const FULL_RISE: i64 = 80_000;

        fn new(ambient: u16) -> Self {
            let ambient = i32::from(ambient) * 1000;
            Self {
                head: ambient,
                mcu: ambient,
                ambient,
            }
        }

        /// Where the head would settle at `level`, taking the power as
        /// proportional to the delta-sigma value, which is a guess.
        fn settle(&self, level: u8) -> i32 {
            let power = i64::from(ramp::output(level).dsm);
            self.ambient + (Self::FULL_RISE * power / i64::from(DSM_TOP)) as i32
        }

        /// One tick at `level`. The head settles with a time constant of a
        /// minute, and the MCU follows it over five seconds.
        fn tick(&mut self, level: u8) {
            self.head += (self.settle(level) - self.head) / (60 * 64);
            self.mcu += (self.head - self.mcu) / (5 * 64);
        }

        fn kelvin(&self) -> u16 {
            ((self.mcu + 500) / 1000) as u16
        }

        /// What the sensor reads, to the sixteenth.
        fn reading(&self) -> u16 {
            ((self.mcu * i32::from(FRACTION) + 500) / 1000) as u16
        }
    }

    /// What a stretch of [`closed_loop`] did.
    #[derive(Debug)]
    struct Stretch {
        /// The hottest the MCU got, in kelvin.
        hottest: u16,
        /// The output's range.
        lowest: u8,
        highest: u8,
        /// How often the output changed direction.
        turns: u16,
    }

    /// Runs the light at `ui` against `head` for `samples` readings, each
    /// off by up to `spread` kelvin.
    fn closed_loop(
        run: &mut Run,
        head: &mut Head,
        ui: u8,
        samples: u16,
        noise: &mut Noise,
        spread: i16,
    ) -> Stretch {
        let mut stretch = Stretch {
            hottest: 0,
            lowest: u8::MAX,
            highest: 0,
            turns: 0,
        };
        let (mut last, mut falling) = (run.output, None);
        for _ in 0..samples {
            let off = noise.next(spread) * FRACTION;
            let reading = head.reading().saturating_add_signed(off);
            run.thermal.sample(reading, run.output);
            for _ in 0..CYCLE_TICKS {
                run.tick(ui);
                head.tick(run.output);
            }
            stretch.hottest = stretch.hottest.max(head.kelvin());
            stretch.lowest = stretch.lowest.min(run.output);
            stretch.highest = stretch.highest.max(run.output);
            if run.output != last {
                let now = run.output < last;
                if falling.is_some_and(|was| was != now) {
                    stretch.turns += 1;
                }
                falling = Some(now);
                last = run.output;
            }
        }
        if std::env::var_os("TRACE").is_some() {
            std::println!("{stretch:?}, ending at {}", run.output);
        }
        stretch
    }

    #[test]
    fn starts_with_no_ceiling() {
        assert_eq!(Thermal::default().ceiling(), CEILING);
    }

    #[test]
    fn the_limit_is_50_degrees_and_the_floor_level_50() {
        assert_eq!(LIMIT, 323);
        assert_eq!(FLOOR, 50);
    }

    #[test]
    fn leaves_it_alone_below_the_limit() {
        let mut run = Run::new();
        for kelvin in [celsius(20), celsius(35), LIMIT - 3] {
            assert_eq!(
                run.hold(kelvin, CEILING, 4 * 60 * 10),
                CEILING,
                "{kelvin} K"
            );
            assert_eq!(run.thermal.ceiling(), CEILING);
        }
    }

    #[test]
    fn steps_down_smoothly_as_it_heats_up() {
        let mut run = Run::new();
        run.hold(celsius(35), CEILING, 20);
        let mut last = CEILING;
        // Warming a degree every two seconds, to 15 °C over the limit.
        for step in 0..8 * 30 {
            let output = run.sample(celsius(35) + step / 8, CEILING);
            assert!(output <= last, "stepped up while heating");
            last = output;
        }
        assert!(last < CEILING, "never stepped down");
        assert!(last >= FLOOR);
    }

    #[test]
    fn starts_stepping_down_before_it_reaches_the_limit() {
        let mut run = Run::new();
        run.hold(celsius(30), CEILING, 20);
        let mut kelvin = celsius(30);
        while run.output == CEILING {
            kelvin += 1;
            run.hold(kelvin, CEILING, 4);
        }
        assert!(kelvin < LIMIT, "waited until {kelvin} K");
    }

    #[test]
    fn sustained_heat_takes_it_to_the_floor_and_no_further() {
        let mut run = Run::new();
        run.hold(celsius(30), CEILING, 20);
        assert_eq!(run.hold(LIMIT + 10, CEILING, 4 * 60), FLOOR);
        assert_eq!(run.hold(LIMIT + 30, CEILING, 4 * 60 * 10), FLOOR);
    }

    #[test]
    fn a_level_below_the_floor_is_never_touched() {
        let mut run = Run::new();
        run.hold(celsius(30), FLOOR - 10, 20);
        assert_eq!(run.hold(LIMIT + 30, FLOOR - 10, 4 * 60), FLOOR - 10);
    }

    #[test]
    fn steps_down_straight_away_from_a_level_under_the_ceiling() {
        let mut run = Run::new();
        let ui = FLOOR + 20;
        run.hold(celsius(30), ui, 20);
        // A believable jump, so it takes a few readings to count.
        run.hold(LIMIT + 5, ui, u16::from(GLITCHES) + 2);
        assert!(
            run.output < ui,
            "waited for the ceiling to come down from the top"
        );
    }

    #[test]
    fn comes_back_up_gently_once_it_cools() {
        let mut run = Run::new();
        run.hold(celsius(30), CEILING, 20);
        assert_eq!(run.hold(LIMIT + 10, CEILING, 4 * 60), FLOOR);
        let (mut last, mut samples) = (FLOOR, 0);
        while run.output < CEILING {
            let output = run.sample(celsius(30), CEILING);
            assert!(output >= last, "stepped down while cool");
            last = output;
            samples += 1;
            assert!(samples < 4 * 60, "took over a minute to recover");
        }
        // No faster than a level a reading.
        assert!(samples >= u16::from(CEILING - FLOOR));
    }

    #[test]
    fn holds_inside_the_window() {
        let mut run = Run::new();
        run.hold(celsius(30), CEILING, 20);
        run.hold(LIMIT + 3, CEILING, 20);
        assert!(run.output < CEILING);
        // Neither hot enough to step down nor cool enough to step up.
        for kelvin in [LIMIT, LIMIT - 1] {
            let held = run.hold(kelvin, CEILING, 20);
            assert_eq!(
                run.hold(kelvin, CEILING, 4 * 60),
                held,
                "moved at {kelvin} K"
            );
        }
    }

    #[test]
    fn noise_below_the_limit_changes_nothing() {
        let mut run = Run::new();
        let mut noise = Noise(7);
        for _ in 0..4 * 60 * 30 {
            let output = run.sample(noisy(LIMIT - 8, &mut noise, 2), CEILING);
            assert_eq!(output, CEILING);
        }
    }

    #[test]
    fn noise_over_the_limit_still_steps_down() {
        let mut run = Run::new();
        let mut noise = Noise(11);
        for _ in 0..4 * 60 {
            run.sample(noisy(LIMIT + 8, &mut noise, 2), CEILING);
        }
        assert_eq!(run.output, FLOOR);
    }

    #[test]
    fn a_steady_reading_between_two_kelvin_is_reached_exactly() {
        let mut thermal = Thermal::default();
        thermal.sample(fraction(celsius(30)), CEILING);
        let reading = fraction(celsius(30)) + 5;
        for _ in 0..8 {
            thermal.sample(reading, CEILING);
        }
        assert_eq!(thermal.smoothed.map(|s| s.now), Some(reading as i16));
    }

    #[test]
    fn a_fraction_of_a_kelvin_over_the_limit_steps_down() {
        // Rounded to whole kelvin this would be the limit exactly, and hold.
        let (mut at, mut over) = (Run::new(), Run::new());
        for _ in 0..4 * 60 {
            at.sample_fraction(fraction(LIMIT), CEILING);
            over.sample_fraction(fraction(LIMIT) + 7, CEILING);
        }
        assert_eq!(at.output, CEILING);
        assert!(over.output < CEILING);
    }

    #[test]
    fn a_slow_rise_moves_the_prediction_steadily() {
        // Warming a sixteenth a reading, the trend should stay the same from
        // one reading to the next once it's filled. Whole kelvin would show
        // a kelvin's step every sixteen readings, and predict from that.
        let mut thermal = Thermal::default();
        let start = fraction(celsius(30));
        let mut trends = std::vec::Vec::new();
        for step in 0..64 {
            thermal.sample(start + step, CEILING);
            let smoothed = thermal.smoothed.as_ref().unwrap();
            let oldest = smoothed.history[usize::from(smoothed.next)];
            trends.push(smoothed.now - oldest);
        }
        let settled = &trends[32..];
        assert!(
            settled.iter().all(|&trend| trend == settled[0]),
            "{trends:?}"
        );
        assert!(settled[0] > 0, "{trends:?}");
    }

    /// A plausible reading in sixteenths, anywhere in the range.
    fn any_plausible(rng: &mut crate::testing::Rng) -> u16 {
        let (low, high) = (*PLAUSIBLE_SIXTEENTHS.start(), *PLAUSIBLE_SIXTEENTHS.end());
        low + rng.next_u16() % (high - low + 1)
    }

    /// Asserts that `now` and every sample of the history are inside the
    /// range the clamps in `follow` and `trend` cap them to, so the clamps
    /// have nothing to change.
    fn assert_in_range(smoothed: &Smoothed, context: &str) {
        let range = NOW_LOW..=NOW_HIGH;
        assert!(range.contains(&smoothed.now), "{context}: {smoothed:?}");
        assert!(
            smoothed.history.iter().all(|sample| range.contains(sample)),
            "{context}: {smoothed:?}"
        );
    }

    /// What `follow` would return with no clamp, worked out in `i32`.
    fn follow_unclamped(smoothed: &Smoothed, reading: u16) -> i32 {
        let now = i32::from(smoothed.now);
        let difference = i32::from(reading) - now;
        now + match difference / 8 {
            0 => difference.signum(),
            step => step,
        }
    }

    /// What `trend` would return with no clamp, worked out in `i32`.
    fn trend_unclamped(smoothed: &Smoothed, now: i16) -> i32 {
        i32::from(now) - i32::from(smoothed.history[usize::from(smoothed.next & HISTORY_MASK)])
    }

    #[test]
    fn smoothed_state_stays_inside_the_plausible_range() {
        let mut rng = crate::testing::Rng::new(0x5EED);
        let (low, high) = (*PLAUSIBLE_SIXTEENTHS.start(), *PLAUSIBLE_SIXTEENTHS.end());

        // `Smoothed` driven directly, so every `follow` and `trend` can be
        // compared with the unclamped sums. The readings are the plausible
        // ones `follow` is given, including jumps from one end to the other.
        let mut slot = None;
        for round in 0..200_u16 {
            // A fresh start every round, from either end or anywhere between.
            let first = match round % 3 {
                0 => low,
                1 => high,
                _ => any_plausible(&mut rng),
            };
            let smoothed = Smoothed::start(&mut slot, first);
            assert_in_range(smoothed, "start");
            for step in 0..100_u16 {
                let reading = match (round + step / 25) % 4 {
                    0 => low,
                    1 => high,
                    _ => any_plausible(&mut rng),
                };
                let want_now = follow_unclamped(smoothed, reading);
                let now = smoothed.follow(reading);
                assert_eq!(
                    i32::from(now),
                    want_now,
                    "follow, round {round}, step {step}"
                );
                let want_trend = trend_unclamped(smoothed, now);
                let trend = smoothed.trend(now);
                assert_eq!(
                    i32::from(trend),
                    want_trend,
                    "trend, round {round}, step {step}"
                );
                assert_in_range(smoothed, "after a sample");
            }
        }

        // And through `Thermal`, with rests (no readings), resets, glitches
        // and jumps believed after they last.
        let mut thermal = Thermal::new();
        for step in 0..20_000_u16 {
            let reading = match rng.next_u16() % 16 {
                0 => low,
                1 => high,
                2 => 0,
                3 => u16::MAX,
                _ => any_plausible(&mut rng),
            };
            match rng.next_u16() % 64 {
                0 => thermal.reset(),
                // A rest, as when the light's off. Nothing's sampled.
                1 => {}
                _ => thermal.sample(reading, CEILING),
            }
            if let Some(smoothed) = &thermal.smoothed {
                assert_in_range(smoothed, &std::format!("step {step}"));
            }
        }
    }

    #[test]
    fn a_fresh_start_fills_the_whole_history_with_the_reading() {
        for reading in [
            *PLAUSIBLE_SIXTEENTHS.start(),
            fraction(celsius(30)) + 5,
            *PLAUSIBLE_SIXTEENTHS.end(),
        ] {
            let mut slot = None;
            let smoothed = Smoothed::start(&mut slot, reading);
            assert_eq!(smoothed.now, reading.cast_signed());
            assert_eq!(smoothed.last, reading);
            assert_eq!(smoothed.next, 0);
            assert_eq!(smoothed.history, [reading.cast_signed(); HISTORY]);
        }
    }

    #[test]
    fn the_plausible_range_and_the_jump_are_the_ones_they_always_were() {
        assert!(!plausible(fraction(233) - 1));
        assert!(plausible(fraction(233)));
        assert!(plausible(fraction(423)));
        assert!(!plausible(fraction(423) + 1));

        let mut thermal = Thermal::default();
        let start = fraction(celsius(30));
        thermal.sample(start, CEILING);
        // Six kelvin away is believed straight off, and a sixteenth more
        // waits for the readings after it.
        thermal.sample(start + JUMP, CEILING);
        assert_eq!(thermal.rejected, 0);
        thermal.sample(start + 2 * JUMP + 1, CEILING);
        assert_eq!(thermal.rejected, 1);
    }

    #[test]
    fn single_glitches_are_ignored() {
        let mut run = Run::new();
        run.hold(celsius(30), CEILING, 20);
        for glitch in [0, 1, 100, LIMIT + 20, 500, 1023, u16::MAX] {
            for _ in 0..GLITCHES {
                assert_eq!(run.sample(glitch, CEILING), CEILING, "{glitch}");
            }
            assert_eq!(run.hold(celsius(30), CEILING, 20), CEILING, "{glitch}");
        }
    }

    #[test]
    fn a_jump_that_lasts_is_believed() {
        let mut run = Run::new();
        run.hold(celsius(30), CEILING, 20);
        assert_eq!(run.hold(LIMIT + 20, CEILING, u16::from(GLITCHES)), CEILING);
        run.hold(LIMIT + 20, CEILING, 4);
        assert!(run.output < CEILING);
    }

    #[test]
    fn a_broken_sensor_steps_down_to_the_floor() {
        for broken in [0, u16::MAX] {
            let mut run = Run::new();
            run.hold(celsius(30), CEILING, 20);
            assert_eq!(run.hold(broken, CEILING, 4 * 10), FLOOR, "{broken}");
            // And one that never worked from the start.
            let mut run = Run::new();
            assert_eq!(run.hold(broken, CEILING, 4 * 10), FLOOR, "{broken}");
        }
    }

    #[test]
    fn anything_at_all_moves_a_level_a_tick_at_most_and_stays_off_the_floor() {
        let mut run = Run::new();
        let mut noise = Noise(13);
        let mut kelvin = celsius(30);
        // An hour of a temperature wandering about, with junk mixed in.
        for _ in 0..4 * 60 * 60 {
            kelvin = kelvin
                .saturating_add_signed(noise.next(1))
                .clamp(celsius(10), LIMIT + 20);
            let reading = match noise.next(50) {
                50 => 0,
                -50 => u16::MAX,
                _ => kelvin,
            };
            // `Run` checks the step every tick.
            assert!(run.sample(reading, CEILING) >= FLOOR);
        }
    }

    /// Turns the light off (no readings while it is) and on again.
    fn off_and_on(run: &mut Run) {
        run.output = 0;
        run.thermal.reset();
    }

    #[test]
    fn a_reset_forgets_the_ceiling_and_the_old_readings() {
        let mut run = Run::new();
        run.hold(celsius(30), CEILING, 20);
        assert_eq!(run.hold(LIMIT + 10, CEILING, 4 * 60), FLOOR);
        off_and_on(&mut run);
        assert_eq!(run.thermal.ceiling(), CEILING);
        // Cooled while off. It's a fresh start, not a jump to ignore, and
        // there's no trend from the hot readings to predict with.
        assert_eq!(run.hold(celsius(30), CEILING, 4 * 60), CEILING);
    }

    #[test]
    fn turning_on_hot_steps_down_from_the_first_readings() {
        let mut run = Run::new();
        run.hold(celsius(30), CEILING, 20);
        off_and_on(&mut run);
        // Turned on at the top while still 10 °C over. The first reading has
        // no history, and it still steps down straight away.
        assert!(run.sample(LIMIT + 10, CEILING) < CEILING);
        let mut samples = 1;
        while run.output > FLOOR {
            run.sample(LIMIT + 10, CEILING);
            samples += 1;
        }
        assert!(samples <= 4 * 10, "took {samples} readings");
    }

    #[test]
    fn turning_on_hot_at_a_low_level_steps_down_from_there() {
        let mut run = Run::new();
        let ui = FLOOR + 30;
        off_and_on(&mut run);
        run.sample(LIMIT + 10, ui);
        // The first step down shows, rather than spending a second or two
        // bringing the ceiling down from the top.
        run.hold(LIMIT + 10, ui, 2);
        assert!(run.output < ui);
    }

    #[test]
    fn the_top_settles_near_the_limit_without_hunting() {
        let mut run = Run::new();
        let mut head = Head::new(celsius(25));
        let mut noise = Noise(3);
        // Ten minutes to get hot and settle, then ten to watch.
        let warming = closed_loop(&mut run, &mut head, CEILING, 4 * 60 * 10, &mut noise, 0);
        assert!(warming.hottest <= LIMIT + 2, "{warming:?}");
        let settled = closed_loop(&mut run, &mut head, CEILING, 4 * 60 * 10, &mut noise, 0);
        assert!(settled.hottest <= LIMIT, "{settled:?}");
        assert!(settled.lowest > FLOOR, "{settled:?}");
        assert!(settled.highest - settled.lowest <= 4, "{settled:?}");
    }

    #[test]
    fn the_top_with_a_noisy_sensor_still_settles() {
        let mut run = Run::new();
        let mut head = Head::new(celsius(25));
        let mut noise = Noise(5);
        let warming = closed_loop(&mut run, &mut head, CEILING, 4 * 60 * 10, &mut noise, 1);
        assert!(warming.hottest <= LIMIT + 2, "{warming:?}");
        let settled = closed_loop(&mut run, &mut head, CEILING, 4 * 60 * 10, &mut noise, 1);
        assert!(settled.hottest <= LIMIT + 1, "{settled:?}");
        assert!(settled.lowest > FLOOR, "{settled:?}");
        assert!(settled.highest - settled.lowest <= 20, "{settled:?}");
    }

    #[test]
    fn a_cool_day_leaves_a_middling_level_alone() {
        let mut run = Run::new();
        let mut head = Head::new(celsius(20));
        let mut noise = Noise(9);
        let ui = FLOOR + 20;
        let stretch = closed_loop(&mut run, &mut head, ui, 4 * 60 * 20, &mut noise, 1);
        assert_eq!((stretch.lowest, stretch.highest), (ui, ui), "{stretch:?}");
    }

    #[test]
    fn every_reading_held_ends_where_the_limit_says() {
        // A steady reading has no trend, so the band under the limit is a
        // hold and the ceiling can only be where it started.
        let limit = LIMIT * FRACTION as u16;
        let mut hot = 0;
        let mut cool = 0;
        for sum in 0..=4 * 1023 {
            let reading = crate::testing::CALIBRATION.sixteenths(sum);
            let too_hot = !plausible(reading) || reading > limit;
            let mut thermal = Thermal::new();
            let mut settled = 0_u32;
            // The slowest reading to give way is a sixteenth over the limit,
            // which takes about sixteen seconds a level.
            for sample in 0..8000_u32 {
                let level = CEILING.min(thermal.ceiling());
                thermal.sample(reading, level);
                for _ in 0..CYCLE_TICKS {
                    let ceiling = thermal.tick();
                    assert!(
                        (FLOOR..=CEILING).contains(&ceiling),
                        "sum {sum} ({reading}), sample {sample}: {ceiling}"
                    );
                }
                settled = match thermal.ceiling() == if too_hot { FLOOR } else { CEILING } {
                    true => settled + 1,
                    false => 0,
                };
                // Stop once it's stayed where it should be for a while.
                if settled >= 50 {
                    break;
                }
            }
            let want = if too_hot { FLOOR } else { CEILING };
            assert_eq!(thermal.ceiling(), want, "sum {sum} ({reading})");
            assert!(settled >= 50, "sum {sum} ({reading}) hadn't settled");
            if too_hot {
                hot += 1;
            } else {
                cool += 1;
            }
        }
        assert!(hot > 0 && cool > 0, "{hot} hot, {cool} cool");
    }
}
