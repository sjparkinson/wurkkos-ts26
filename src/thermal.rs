use crate::gradual::Ceiling;
use crate::ramp::CEILING;
use crate::sensors::CYCLE_TICKS;
use crate::{u16_from, usize_from};

#[expect(
    clippy::as_conversions,
    reason = "widening u8 to u16 in a const fn, where From can't be called"
)]
#[must_use]
pub const fn celsius(degrees: u8) -> u16 {
    degrees as u16 + 273
}

// Every number from here down is unmeasured on this torch.
pub const LIMIT: u16 = celsius(50);
pub const FLOOR: u8 = 50;

const _: () = assert!(FLOOR <= CEILING);

const FRACTION: i16 = 16;
const HISTORY_MASK: u8 = 7;
const HISTORY: usize = usize_from(HISTORY_MASK) + 1;
const LOOKAHEAD: i16 = 4;
const WINDOW: i16 = 2 * FRACTION;
const WARMING: i16 = FRACTION;
const MAX_TREND: i16 = 64 * FRACTION;
const DOWN_PRESSURE: i16 = 4 * FRACTION;
const UP_PRESSURE: i16 = 16 * FRACTION;
const MAX_STEP_DOWN: u8 = 8;
const DOWN_EVERY: u8 = 2;
const FAST_ABOVE: u8 = 120;
const FAST_GAIN: i16 = 2;
const UP_EVERY: u8 = 4;
const PLAUSIBLE: core::ops::RangeInclusive<u16> = 233..=423;

const LIMIT_SIXTEENTHS: i16 = LIMIT.cast_signed() * FRACTION;

const _: () = assert!(LIMIT <= *PLAUSIBLE.end());

#[must_use]
pub fn plausible(sixteenths: u16) -> bool {
    PLAUSIBLE_SIXTEENTHS.contains(&sixteenths)
}

const PLAUSIBLE_SIXTEENTHS: core::ops::RangeInclusive<u16> =
    *PLAUSIBLE.start() * FRACTION.cast_unsigned()..=*PLAUSIBLE.end() * FRACTION.cast_unsigned();

const NOW_LOW: i16 = PLAUSIBLE_SIXTEENTHS.start().cast_signed();
const NOW_HIGH: i16 = PLAUSIBLE_SIXTEENTHS.end().cast_signed();

const _: () = {
    let swing = MAX_TREND * LOOKAHEAD;
    assert!(NOW_HIGH.checked_add(swing).is_some());
    assert!(NOW_LOW.checked_sub(swing + LIMIT_SIXTEENTHS).is_some());
};

const JUMP: u16 = 6 * FRACTION.cast_unsigned();
const GLITCHES: u8 = 3;

const _: () = assert!(u16_from(MAX_STEP_DOWN) * u16_from(DOWN_EVERY) <= u16_from(CYCLE_TICKS));

const _: () = assert!(HISTORY.is_power_of_two());

#[derive(Clone, Debug)]
struct Smoothed {
    now: i16,
    history: [i16; HISTORY],
    next: u8,
    last: u16,
}

impl Smoothed {
    /// Building the history whole pulls in `memcpy`, which is bigger than filling it in place.
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
    smoothed: Option<Smoothed>,
    rejected: u8,
    pressure: i16,
}

impl Default for Thermal {
    fn default() -> Self {
        Self::new()
    }
}

impl Thermal {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            ceiling: Ceiling::new(DOWN_EVERY, UP_EVERY),
            smoothed: None,
            rejected: 0,
            pressure: 0,
        }
    }

    #[must_use]
    pub const fn ceiling(&self) -> u8 {
        self.ceiling.level()
    }

    pub const fn reset(&mut self) {
        *self = Self::new();
    }

    pub const fn tick(&mut self) -> u8 {
        self.ceiling.tick()
    }

    pub fn sample(&mut self, reading: u16, level: u8) {
        if !plausible(reading) {
            self.rejected = self.rejected.saturating_add(1);
            if self.rejected > GLITCHES {
                self.step_down(level, MAX_STEP_DOWN);
            }
            return;
        }
        let (smoothed, smooth) = match &mut self.smoothed {
            Some(smoothed) if reading.abs_diff(smoothed.last) <= JUMP => {
                let smooth = smoothed.follow(reading);
                (smoothed, smooth)
            }
            Some(_) if self.rejected < GLITCHES => {
                self.rejected = self.rejected.saturating_add(1);
                return;
            }
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
            let push = if level > FAST_ABOVE {
                over.saturating_mul(FAST_GAIN)
            } else {
                over
            };
            let pressure = self.pressure.max(0).saturating_add(push);
            let steps = (pressure / DOWN_PRESSURE).min(i16::from(MAX_STEP_DOWN));
            self.pressure = pressure
                .saturating_sub(steps.saturating_mul(DOWN_PRESSURE))
                .min(DOWN_PRESSURE - 1);
            if steps > 0 {
                let steps = u8::try_from(steps).unwrap_or(MAX_STEP_DOWN);
                self.step_down(level, steps);
            }
        } else if over < -WINDOW && trend < WARMING {
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
            self.pressure = 0;
        }
    }

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
    extern crate std;
    use super::*;
    use crate::ramp::{self, DSM_TOP};
    use crate::sensors::FULL_SCALE;

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

        fn sample(&mut self, kelvin: u16, ui: u8) -> u8 {
            self.sample_fraction(fraction(kelvin), ui)
        }

        fn sample_fraction(&mut self, reading: u16, ui: u8) -> u8 {
            self.thermal.sample(reading, self.output);
            for _ in 0..CYCLE_TICKS {
                self.tick(ui);
            }
            self.output
        }

        fn tick(&mut self, ui: u8) {
            let output = ui.min(self.thermal.tick());
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

    struct Noise(u32);

    impl Noise {
        fn next(&mut self, spread: i16) -> i16 {
            self.0 = self.0.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            ((self.0 >> 16) % (2 * spread as u32 + 1)) as i16 - spread
        }
    }

    fn noisy(kelvin: u16, noise: &mut Noise, spread: i16) -> u16 {
        kelvin.saturating_add_signed(noise.next(spread))
    }

    fn fraction(kelvin: u16) -> u16 {
        kelvin.saturating_mul(FRACTION as u16)
    }

    /// Unmeasured: a guess at a head, in thousandths of a kelvin.
    #[expect(
        clippy::struct_field_names,
        reason = "the head's temperature, beside the MCU's and the ambient"
    )]
    struct Head {
        head: i32,
        mcu: i32,
        ambient: i32,
    }

    impl Head {
        const FULL_RISE: i64 = 80_000;

        fn new(ambient: u16) -> Self {
            let ambient = i32::from(ambient) * 1000;
            Self {
                head: ambient,
                mcu: ambient,
                ambient,
            }
        }

        fn settle(&self, level: u8) -> i32 {
            let power = i64::from(ramp::output(level).dsm);
            self.ambient + (Self::FULL_RISE * power / i64::from(DSM_TOP)) as i32
        }

        fn tick(&mut self, level: u8) {
            self.head += (self.settle(level) - self.head) / (60 * 64);
            self.mcu += (self.head - self.mcu) / (5 * 64);
        }

        fn kelvin(&self) -> u16 {
            ((self.mcu + 500) / 1000) as u16
        }

        fn reading(&self) -> u16 {
            ((self.mcu * i32::from(FRACTION) + 500) / 1000) as u16
        }
    }

    #[derive(Debug)]
    struct Stretch {
        hottest: u16,
        lowest: u8,
        highest: u8,
        turns: u16,
    }

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
        assert!(samples >= u16::from(CEILING - FLOOR));
    }

    #[test]
    fn holds_inside_the_window() {
        let mut run = Run::new();
        run.hold(celsius(30), CEILING, 20);
        run.hold(LIMIT + 3, CEILING, 20);
        assert!(run.output < CEILING);
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

    fn any_plausible(rng: &mut crate::testing::Rng) -> u16 {
        let (low, high) = (*PLAUSIBLE_SIXTEENTHS.start(), *PLAUSIBLE_SIXTEENTHS.end());
        low + rng.next_u16() % (high - low + 1)
    }

    fn assert_in_range(smoothed: &Smoothed, context: &str) {
        let range = NOW_LOW..=NOW_HIGH;
        assert!(range.contains(&smoothed.now), "{context}: {smoothed:?}");
        assert!(
            smoothed.history.iter().all(|sample| range.contains(sample)),
            "{context}: {smoothed:?}"
        );
    }

    fn follow_unclamped(smoothed: &Smoothed, reading: u16) -> i32 {
        let now = i32::from(smoothed.now);
        let difference = i32::from(reading) - now;
        now + match difference / 8 {
            0 => difference.signum(),
            step => step,
        }
    }

    fn trend_unclamped(smoothed: &Smoothed, now: i16) -> i32 {
        i32::from(now) - i32::from(smoothed.history[usize::from(smoothed.next & HISTORY_MASK)])
    }

    #[test]
    fn smoothed_state_stays_inside_the_plausible_range() {
        let mut rng = crate::testing::Rng::new(0x5EED);
        let (low, high) = (*PLAUSIBLE_SIXTEENTHS.start(), *PLAUSIBLE_SIXTEENTHS.end());

        let mut slot = None;
        for round in 0..200_u16 {
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
        thermal.sample(start + fraction(6), CEILING);
        assert_eq!(thermal.rejected, 0);
        thermal.sample(start + 2 * fraction(6) + 1, CEILING);
        assert_eq!(thermal.rejected, 1);
    }

    fn limit_plus(sixteenths: i16) -> u16 {
        (LIMIT_SIXTEENTHS + sixteenths).cast_unsigned()
    }

    fn cooled_at_the_floor(reading: u16) -> Thermal {
        let mut thermal = Thermal::default();
        for _ in 0..4 * 10 {
            thermal.sample(fraction(LIMIT + 10), CEILING);
        }
        assert_eq!(thermal.ceiling.target(), FLOOR);
        for _ in 0..=GLITCHES {
            thermal.sample(reading, CEILING);
        }
        assert_eq!(
            thermal.smoothed.as_ref().map(|s| s.now),
            Some(reading as i16)
        );
        thermal
    }

    fn readings_to_step_up(thermal: &mut Thermal, reading: u16) -> u16 {
        let from = thermal.ceiling.target();
        let mut readings = 0;
        while thermal.ceiling.target() == from {
            thermal.sample(reading, CEILING);
            readings += 1;
            assert!(readings <= 4 * 60 * 5, "never stepped up from {from}");
        }
        assert_eq!(thermal.ceiling.target(), from + 1, "more than a level");
        readings
    }

    const PLAIN: u8 = FAST_ABOVE;

    fn after(stepped: u8, level: u8) -> u8 {
        match stepped {
            0 => CEILING,
            stepped => level - stepped,
        }
    }

    #[test]
    fn the_rate_down_follows_how_far_over_the_limit_it_is() {
        for (over, every) in [(1, 64), (FRACTION, 4), (4 * FRACTION, 1)] {
            let mut thermal = Thermal::default();
            for step in 1..=3 {
                for reading in 1..=every {
                    thermal.sample(limit_plus(over), PLAIN);
                    let stepped = if reading == every { step } else { step - 1 };
                    assert_eq!(
                        thermal.ceiling.target(),
                        after(stepped, PLAIN),
                        "{over} over, step {step}, reading {reading}"
                    );
                }
            }
        }
    }

    #[test]
    fn above_level_120_the_rate_down_doubles() {
        assert_eq!((FAST_ABOVE, FAST_GAIN), (120, 2));
        for (level, every) in [(PLAIN, 4), (PLAIN + 1, 2), (CEILING, 2)] {
            let mut thermal = Thermal::default();
            for reading in 1..=every {
                thermal.sample(limit_plus(FRACTION), level);
                let stepped = u8::from(reading == every);
                assert_eq!(
                    thermal.ceiling.target(),
                    after(stepped, level),
                    "level {level}, reading {reading}"
                );
            }
        }
    }

    #[test]
    fn a_step_down_is_eight_levels_at_most_and_the_rest_is_not_saved_up() {
        let mut thermal = Thermal::default();
        thermal.sample(fraction(LIMIT + 40), CEILING);
        assert_eq!(thermal.ceiling.target(), CEILING - MAX_STEP_DOWN);
        assert!(thermal.pressure < DOWN_PRESSURE, "{}", thermal.pressure);
        thermal.sample(fraction(LIMIT + 40), CEILING);
        assert_eq!(thermal.ceiling.target(), CEILING - 2 * MAX_STEP_DOWN);
    }

    #[test]
    fn a_reading_that_buys_no_step_leaves_the_ceiling_alone() {
        let mut thermal = Thermal::default();
        thermal.sample(limit_plus(1), FLOOR + 20);
        assert_eq!(thermal.ceiling(), CEILING);
        assert_eq!(thermal.ceiling.target(), CEILING);
    }

    #[test]
    fn well_over_the_limit_it_steps_down_even_while_cooling() {
        let mut thermal = Thermal::default();
        thermal.sample(fraction(LIMIT + 10), CEILING);
        let before = thermal.ceiling.target();
        thermal.sample(fraction(LIMIT + 9), CEILING);
        assert!(thermal.ceiling.target() < before);
    }

    #[test]
    fn cooling_from_the_top_of_the_window_holds_and_keeps_the_pressure() {
        let mut thermal = Thermal::default();
        for _ in 0..5 {
            thermal.sample(limit_plus(WINDOW + 1), PLAIN);
        }
        assert_eq!(thermal.ceiling.target(), PLAIN - 2);
        thermal.sample(limit_plus(WINDOW), PLAIN);
        assert_eq!(thermal.ceiling.target(), PLAIN - 2);
        thermal.sample(limit_plus(WINDOW + 1), PLAIN);
        assert_eq!(thermal.ceiling.target(), PLAIN - 3);
    }

    #[test]
    fn a_prediction_on_either_edge_of_the_window_holds_and_forgets_the_pressure() {
        let mut thermal = Thermal::default();
        thermal.sample(limit_plus(50), PLAIN);
        assert!(thermal.pressure > 0);
        thermal.sample(limit_plus(-30), PLAIN);
        let smoothed = thermal.smoothed.as_ref().unwrap();
        assert_eq!(smoothed.now, LIMIT_SIXTEENTHS + 40);
        assert_eq!(thermal.pressure, 0);
        assert_eq!(thermal.ceiling.target(), CEILING);

        let mut thermal = cooled_at_the_floor(limit_plus(-WINDOW - 5));
        for _ in 0..10 {
            thermal.sample(limit_plus(-WINDOW - 5), CEILING);
        }
        assert!(thermal.pressure < 0);
        thermal.sample(limit_plus(-WINDOW - 4), CEILING);
        assert_eq!(thermal.pressure, 0);
        assert_eq!(thermal.ceiling.target(), FLOOR);
    }

    #[test]
    fn the_rate_up_follows_how_far_under_the_window_it_is() {
        for (under, every) in [
            (1, 256),
            (FRACTION, 16),
            (16 * FRACTION, 1),
            (25 * FRACTION, 1),
        ] {
            let reading = limit_plus(-WINDOW - under);
            let mut thermal = cooled_at_the_floor(reading);
            readings_to_step_up(&mut thermal, reading);
            for step in 1..=3 {
                assert_eq!(
                    readings_to_step_up(&mut thermal, reading),
                    every,
                    "{under} under, step {step}"
                );
            }
        }
    }

    #[test]
    fn the_pressure_left_over_from_a_step_up_counts_towards_the_next() {
        let reading = limit_plus(-WINDOW - 24);
        let mut thermal = cooled_at_the_floor(reading);
        readings_to_step_up(&mut thermal, reading);
        let from = thermal.ceiling.target();
        for _ in 0..4 * 16 {
            thermal.sample(reading, CEILING);
        }
        assert_eq!(thermal.ceiling.target(), from + 6);
    }

    #[test]
    fn two_kelvin_under_the_limit_is_still_inside_the_window() {
        let reading = fraction(LIMIT - 2);
        let mut thermal = cooled_at_the_floor(reading);
        for _ in 0..4 * 60 * 5 {
            thermal.sample(reading, CEILING);
        }
        assert_eq!(thermal.ceiling.target(), FLOOR);
    }

    #[test]
    fn warming_a_kelvin_every_two_seconds_blocks_stepping_up() {
        let start = fraction(LIMIT - 30);
        let mut thermal = cooled_at_the_floor(start);
        let mut reading = start;
        let mut warm = |thermal: &mut Thermal, readings: u16| {
            for _ in 0..readings {
                reading += 2;
                thermal.sample(reading, CEILING);
            }
        };
        warm(&mut thermal, 40);
        let smoothed = thermal.smoothed.as_ref().unwrap();
        let oldest = smoothed.history[usize::from(smoothed.next)];
        assert_eq!(smoothed.now - oldest, 7 * FRACTION / 8);
        let held = thermal.ceiling.target();
        assert!(held < CEILING);
        warm(&mut thermal, 20);
        assert_eq!(thermal.ceiling.target(), held);
        let reading = reading;
        for _ in 0..HISTORY * 2 {
            thermal.sample(reading, CEILING);
        }
        assert!(thermal.ceiling.target() > held);
    }

    #[test]
    fn a_fast_rise_steps_down_long_before_the_smoothed_temperature_gets_there() {
        let mut thermal = Thermal::default();
        let start = fraction(LIMIT - 30);
        thermal.sample(start, PLAIN);
        for step in 1..=5 {
            assert_eq!(thermal.ceiling.target(), CEILING);
            thermal.sample(start + step * fraction(6), PLAIN);
        }
        let smoothed = thermal.smoothed.as_ref().unwrap();
        assert!(smoothed.now < LIMIT_SIXTEENTHS - 20 * FRACTION);
        assert!(thermal.ceiling.target() < CEILING);
    }

    #[test]
    fn the_fourth_glitch_in_a_row_is_believed() {
        let mut run = Run::new();
        run.hold(celsius(30), CEILING, 20);
        assert_eq!(run.hold(LIMIT + 20, CEILING, u16::from(GLITCHES)), CEILING);
        assert_eq!(run.thermal.ceiling.target(), CEILING);
        assert!(run.sample(LIMIT + 20, CEILING) < CEILING);
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
            let mut run = Run::new();
            assert_eq!(run.hold(broken, CEILING, 4 * 10), FLOOR, "{broken}");
        }
    }

    #[test]
    fn anything_at_all_moves_a_level_a_tick_at_most_and_stays_off_the_floor() {
        let mut run = Run::new();
        let mut noise = Noise(13);
        let mut kelvin = celsius(30);
        for _ in 0..4 * 60 * 60 {
            kelvin = kelvin
                .saturating_add_signed(noise.next(1))
                .clamp(celsius(10), LIMIT + 20);
            let reading = match noise.next(50) {
                50 => 0,
                -50 => u16::MAX,
                _ => kelvin,
            };
            assert!(run.sample(reading, CEILING) >= FLOOR);
        }
    }

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
        assert_eq!(run.hold(celsius(30), CEILING, 4 * 60), CEILING);
    }

    #[test]
    fn turning_on_hot_steps_down_from_the_first_readings() {
        let mut run = Run::new();
        run.hold(celsius(30), CEILING, 20);
        off_and_on(&mut run);
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
        run.hold(LIMIT + 10, ui, 2);
        assert!(run.output < ui);
    }

    #[test]
    fn the_top_settles_near_the_limit_without_hunting() {
        let mut run = Run::new();
        let mut head = Head::new(celsius(25));
        let mut noise = Noise(3);
        let warming = closed_loop(&mut run, &mut head, CEILING, 4 * 60 * 10, &mut noise, 0);
        assert!(warming.hottest <= LIMIT + 2, "{warming:?}");
        assert!(warming.lowest > FLOOR + 10, "{warming:?}");
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
        assert!(warming.lowest > FLOOR + 10, "{warming:?}");
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
        let limit = LIMIT * FRACTION as u16;
        let mut hot = 0;
        let mut cool = 0;
        for sum in 0..=FULL_SCALE {
            let reading = crate::testing::CALIBRATION.sixteenths(sum);
            let too_hot = !plausible(reading) || reading > limit;
            let mut thermal = Thermal::new();
            let mut settled = 0_u32;
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
                let target = if too_hot { FLOOR } else { CEILING };
                settled = if thermal.ceiling() == target {
                    settled + 1
                } else {
                    0
                };
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
