//! What the cell and the temperature allow the UI's level to be.
//!
//! A cell that can't be read is a fault that keeps the light off, since the
//! cell has no protection of its own.

use crate::battery::{self, Protection, Verdict};
use crate::ramp::CEILING;
use crate::selftest::Faults;
use crate::sensors::{Convert, Reading, Schedule};
use crate::temp::Calibration;
use crate::thermal::Thermal;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Health {
    Good,
    Flat,
    Fault,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Step {
    pub level: u8,
    pub off: bool,
    pub unloaded: Option<u16>,
}

#[derive(Clone, Debug)]
pub struct Limits {
    schedule: Schedule,
    calibration: Calibration,
    protection: Protection,
    thermal: Thermal,
    on: bool,
    level: u8,
    /// A conversion starts before its tick's output is set, so a reading due
    /// now ran across the last two ticks.
    lit_before: bool,
    convert: Option<Convert>,
    fault: bool,
}

impl Limits {
    pub const fn new(calibration: Calibration, faults: Faults) -> Self {
        Self {
            schedule: Schedule::new(),
            calibration,
            protection: Protection::new(),
            thermal: Thermal::new(),
            on: false,
            level: 0,
            lit_before: false,
            convert: None,
            fault: faults.any(),
        }
    }

    pub const fn convert(&self) -> Option<Convert> {
        self.convert
    }

    pub const fn level(&self) -> u8 {
        self.level
    }

    pub const fn ceilings(&self) -> (u8, u8) {
        (self.protection.ceiling(), self.thermal.ceiling())
    }

    pub const fn health(&self) -> Health {
        if self.fault {
            Health::Fault
        } else if self.protection.flat() {
            Health::Flat
        } else {
            Health::Good
        }
    }

    /// Starts no conversion, since one running into power-down comes back on
    /// waking with an invalid result (DS40002204A §30.3.5).
    pub const fn rest(&mut self) {
        self.schedule.rest();
        self.protection.rest();
        self.convert = None;
        self.on = false;
        self.level = 0;
    }

    pub fn tick(&mut self, ui: u8, result: Option<u16>) -> Step {
        let on = ui != 0;
        if on && !self.on {
            self.thermal.reset();
        }
        self.on = on;
        let ui = ui.min(CEILING);
        let level = match on {
            true => ui
                .min(self.thermal.ceiling())
                .min(self.protection.ceiling())
                .max(self.level),
            false => 0,
        };

        let (reading, convert) = self.schedule.tick(result, on);
        self.convert = convert;
        let mut unloaded = None;
        match reading {
            Some(Reading::Voltage(raw)) => {
                let centivolts = battery::centivolts(raw);
                if level == 0 && self.level == 0 && !self.lit_before {
                    unloaded = Some(centivolts);
                }
                if self.protection.sample(centivolts, level) == Verdict::Blind {
                    self.fault = true;
                }
            }
            Some(Reading::Temperature(raw)) => {
                let sixteenths = self.calibration.sixteenths(raw);
                if on {
                    self.thermal.sample(sixteenths, level);
                }
            }
            None => {}
        }
        if self.fault {
            self.protection.fail(level);
        }

        let thermal = self.thermal.tick();
        let battery = self.protection.tick();
        let fault = self.fault && self.level <= 1;
        let flat = !self.fault && self.protection.flat();
        let off = on && (fault || flat);
        self.lit_before = self.level > 0;
        self.level = if on && !off {
            ui.min(thermal).min(battery)
        } else {
            0
        };
        Step {
            level: self.level,
            off,
            unloaded,
        }
    }
}

#[cfg(test)]
mod tests {
    extern crate std;

    use super::*;
    use crate::TICKS_PER_SECOND;
    use crate::ramp::LEVELS;
    use crate::sensors::{CYCLE_TICKS, FULL_SCALE};
    use crate::testing::{Adc, CALIBRATION, temperature_reading, voltage_reading};
    use crate::thermal::{self, FLOOR};
    use std::vec::Vec;

    const FAILED: Faults = Faults {
        hang: true,
        ..Faults::NONE
    };

    struct Run {
        limits: Limits,
        adc: Adc,
        ui: u8,
        uis: Vec<u8>,
        levels: Vec<u8>,
        offs: usize,
    }

    impl Run {
        fn new() -> Self {
            Self::with(Faults::NONE)
        }

        fn with(faults: Faults) -> Self {
            Self {
                limits: Limits::new(CALIBRATION, faults),
                adc: Adc::new(),
                ui: 0,
                uis: Vec::new(),
                levels: Vec::new(),
                offs: 0,
            }
        }

        fn tick(&mut self) -> u8 {
            let ui = self.ui;
            let step = self.limits.tick(ui, self.adc.result());
            self.adc.start(self.limits.convert());
            if step.off {
                self.offs += 1;
                self.ui = 0;
            }
            let steady = self.uis.last() == Some(&ui) && ui != 0 && !step.off;
            if let (Some(&last), true) = (self.levels.last(), steady) {
                assert!(
                    last.abs_diff(step.level) <= 1,
                    "jumped {last} → {}",
                    step.level
                );
            }
            self.uis.push(ui);
            self.levels.push(step.level);
            step.level
        }

        fn ticks(&mut self, ticks: u32) -> u8 {
            for _ in 0..ticks {
                self.tick();
            }
            *self.levels.last().unwrap()
        }

        fn secs(&mut self, seconds: u32) -> u8 {
            self.ticks(seconds * u32::from(TICKS_PER_SECOND))
        }
    }

    fn celsius(degrees: u8) -> u16 {
        temperature_reading(thermal::celsius(degrees))
    }

    #[test]
    fn a_good_cell_and_a_cool_head_leave_the_ui_alone() {
        let mut run = Run::new();
        assert_eq!(run.secs(1), 0, "dark from off");
        for ui in [1, 50, CEILING] {
            run.ui = ui;
            assert_eq!(run.secs(60), ui);
            assert!(run.levels.iter().rev().take(59 * 64).all(|&l| l == ui));
        }
        assert_eq!(run.offs, 0);
        assert_eq!(run.limits.health(), Health::Good);
    }

    #[test]
    fn nothing_runs_above_the_ramps_ceiling() {
        let mut run = Run::new();
        run.ui = LEVELS;
        assert_eq!(run.secs(60), CEILING);
        assert!(run.levels.iter().all(|&l| l <= CEILING));
    }

    #[test]
    fn a_hot_head_fades_down_to_the_floor() {
        let mut run = Run::new();
        run.ui = CEILING;
        run.secs(2);
        run.adc.temperature = celsius(70);
        let level = run.secs(2);
        assert!(level < CEILING && level > FLOOR, "{level}");
        assert_eq!(run.secs(120), FLOOR);
        assert_eq!(run.limits.ceilings().1, FLOOR);
        assert_eq!(run.offs, 0);
    }

    #[test]
    fn a_hot_head_never_goes_under_the_floor_and_comes_back_up_as_it_cools() {
        let mut run = Run::new();
        run.ui = CEILING;
        run.adc.temperature = celsius(80);
        assert_eq!(run.secs(120), FLOOR);
        assert!(run.levels.iter().all(|&l| l == 0 || l >= FLOOR));
        run.adc.temperature = celsius(25);
        let level = run.secs(5);
        assert!(level > FLOOR && level < CEILING, "{level}");
        assert_eq!(run.secs(120), CEILING);
        assert_eq!(run.limits.ceilings().1, CEILING);
        assert_eq!(run.offs, 0);
    }

    #[test]
    fn heat_leaves_a_level_under_the_floor_alone() {
        let mut run = Run::new();
        run.ui = FLOOR - 10;
        run.adc.temperature = celsius(80);
        assert_eq!(run.secs(120), FLOOR - 10);
    }

    #[test]
    fn a_low_cell_steps_down_gradually_then_turns_off() {
        let mut run = Run::new();
        run.adc.voltage = voltage_reading(295);
        run.ui = 50;
        assert_eq!(run.secs(4), 50);
        assert_eq!(run.secs(2), 35);
        assert_eq!(run.secs(5), 20);
        for level in 35..50 {
            assert!(run.levels.contains(&level), "{level}");
        }
        run.secs(30);
        assert_eq!(run.offs, 1);
        assert_eq!(*run.levels.last().unwrap(), 0);
        assert_eq!(run.limits.health(), Health::Flat);
    }

    #[test]
    fn a_flat_cell_fades_down_the_ramp_before_it_turns_off() {
        let mut run = Run::new();
        run.ui = CEILING;
        run.secs(1);
        run.adc.voltage = voltage_reading(280);
        assert_eq!(run.secs(4), CEILING);
        assert_eq!(run.offs, 0);
        run.secs(10);
        assert_eq!(run.offs, 1);
        let lit: Vec<u8> = run
            .levels
            .iter()
            .rev()
            .skip_while(|&&l| l == 0)
            .copied()
            .collect();
        let bottom = lit.iter().take_while(|&&l| l == 1).count();
        let hold = usize::from(battery::HOLD_SAMPLES);
        let sample = usize::from(CYCLE_TICKS);
        assert!(
            (hold - 1) * sample <= bottom && bottom <= (hold + 1) * sample,
            "{bottom} ticks at level 1"
        );
        assert_eq!(lit[bottom], 2, "faded the whole way");
    }

    #[test]
    fn a_voltage_reading_on_the_tick_it_turns_on_counts() {
        let sample = usize::from(CYCLE_TICKS);
        let mut run = Run::new();
        run.adc.voltage = voltage_reading(280);
        let on_at = 4 * sample + 2;
        run.ticks(on_at as u32);
        run.ui = 1;
        run.ticks(1);
        assert_eq!(run.levels[on_at], 1);
        run.secs(2);
        let off_at = on_at + (usize::from(battery::HOLD_SAMPLES) - 1) * sample;
        assert!(run.levels[on_at..off_at].iter().all(|&l| l == 1));
        assert_eq!(run.levels[off_at], 0);
        assert_eq!(run.offs, 1);
    }

    #[test]
    fn a_flat_cell_stays_off_until_it_reads_as_charged() {
        let mut run = Run::new();
        run.adc.voltage = voltage_reading(280);
        run.ui = 50;
        run.secs(6);
        assert_eq!(run.offs, 1);
        run.ui = 50;
        assert_eq!(run.ticks(1), 0);
        assert_eq!(run.offs, 2);
        run.adc.voltage = voltage_reading(battery::CHARGED_AT - 1);
        run.secs(10);
        assert_eq!(run.limits.health(), Health::Flat);
        run.adc.voltage = voltage_reading(380);
        run.ticks(u32::from(CYCLE_TICKS));
        assert_eq!(run.limits.health(), Health::Good);
        run.ui = 50;
        assert_eq!(run.ticks(1), 50, "as if new, with no ceiling");
        assert_eq!(run.secs(30), 50);
    }

    #[test]
    fn garbage_voltages_fade_it_out_and_latch_a_fault() {
        for garbage in [0, FULL_SCALE, 1] {
            let mut run = Run::new();
            run.ui = CEILING;
            run.secs(2);
            run.adc.voltage = garbage;
            run.ticks(3 * u32::from(CYCLE_TICKS));
            assert_eq!(run.ticks(1), CEILING, "{garbage}");
            assert_eq!(run.limits.health(), Health::Good, "{garbage}");
            run.secs(3);
            assert_eq!(run.offs, 1, "{garbage}");
            assert_eq!(run.limits.health(), Health::Fault, "{garbage}");
            let lit: Vec<u8> = run.levels.iter().copied().filter(|&l| l > 0).collect();
            for level in 1..CEILING {
                assert!(lit.contains(&level), "{garbage}: faded through {level}");
            }
            assert_eq!(lit.last(), Some(&1), "{garbage}");
            run.adc.voltage = voltage_reading(380);
            run.secs(10);
            run.ui = CEILING;
            assert_eq!(run.ticks(1), 0, "{garbage}");
            assert_eq!(run.offs, 2, "{garbage}");
            assert_eq!(run.limits.health(), Health::Fault, "{garbage}");
        }
    }

    #[test]
    fn a_low_cell_during_a_faults_fade_still_fades_a_level_a_tick() {
        let mut run = Run::new();
        run.ui = CEILING;
        run.adc.voltage = voltage_reading(295);
        run.secs(4);
        run.adc.voltage = 0;
        while run.limits.health() == Health::Good {
            run.tick();
        }
        run.adc.voltage = voltage_reading(295);
        run.secs(5);
        assert_eq!(run.offs, 1);
        let lit: Vec<u8> = run.levels.iter().copied().filter(|&l| l > 0).collect();
        for level in 1..CEILING {
            assert!(lit.contains(&level), "faded through {level}");
        }
        assert_eq!(lit.last(), Some(&1));
    }

    #[test]
    fn ramping_down_through_a_fade_still_moves_a_level_a_tick() {
        let mut run = Run::new();
        run.ui = CEILING;
        run.secs(2);
        run.adc.voltage = 0;
        while run.limits.health() == Health::Good {
            run.tick();
        }
        let mut last = run.limits.level();
        assert_eq!(last, CEILING - 1, "the fade's first step");
        while last > 0 {
            if run.ui > CEILING / 2 {
                run.ui -= 1;
            }
            let level = run.tick();
            assert_eq!(level, last - 1, "from {last} with the UI at {}", run.ui);
            last = level;
        }
        assert_eq!(run.offs, 1);
    }

    #[test]
    fn good_readings_during_the_fade_still_see_it_out() {
        let mut run = Run::new();
        run.ui = CEILING;
        run.secs(2);
        run.adc.voltage = 0;
        while run.limits.health() == Health::Good {
            run.tick();
        }
        run.adc.voltage = voltage_reading(420);
        run.secs(30);
        assert_eq!(run.offs, 1);
        let out = run.levels.iter().rposition(|&l| l > 0).unwrap();
        assert!(run.levels[out..].len() > 25 * 64, "out within 5 s");
        assert!(run.limits.ceilings().0 <= 1);
    }

    #[test]
    fn garbage_voltages_with_the_light_off_are_a_fault_too() {
        let mut run = Run::new();
        run.adc.voltage = 0;
        assert_eq!(run.secs(2), 0);
        assert_eq!(run.limits.health(), Health::Fault);
        run.ui = CEILING;
        assert_eq!(run.ticks(1), 0);
        assert_eq!(run.offs, 1);
    }

    #[test]
    fn a_glitch_or_three_is_no_fault() {
        let mut run = Run::new();
        run.ui = CEILING;
        for _ in 0..60 {
            run.adc.voltage = 0;
            run.ticks(3 * u32::from(CYCLE_TICKS));
            run.adc.voltage = voltage_reading(380);
            run.ticks(u32::from(CYCLE_TICKS));
        }
        assert_eq!(run.limits.health(), Health::Good);
        assert_eq!(*run.levels.last().unwrap(), CEILING);
        assert_eq!(run.offs, 0);
    }

    #[test]
    fn a_dead_adc_is_a_fault() {
        let mut run = Run::new();
        run.ui = CEILING;
        run.secs(1);
        run.adc.voltage = 0;
        run.adc.temperature = 0;
        assert_eq!(run.secs(30), 0);
        assert_eq!(run.limits.health(), Health::Fault);
        assert_eq!(run.offs, 1);
    }

    /// A known gap (docs/safety.md H3, H12), pinned so that closing it has to
    /// change this test.
    #[test]
    fn a_plausible_voltage_that_never_changes_keeps_the_light_on() {
        let mut run = Run::new();
        assert_eq!(battery::centivolts(run.adc.voltage), 380);
        run.ui = 100;
        run.secs(1);
        for minute in 0..60 {
            assert_eq!(run.secs(60), 100, "minute {minute}");
            assert_eq!(run.limits.ceilings().0, CEILING, "minute {minute}");
        }
        assert!(run.levels.iter().skip(1).all(|&l| l == 100));
        assert_eq!(run.offs, 0);
    }

    /// A known gap (docs/safety.md H4), pinned so that closing it has to
    /// change this test.
    #[test]
    fn a_plausible_temperature_that_never_changes_never_steps_down() {
        let mut run = Run::new();
        assert_eq!(run.adc.temperature, celsius(25));
        run.ui = CEILING;
        run.secs(1);
        for minute in 0..60 {
            assert_eq!(run.secs(60), CEILING, "minute {minute}");
            assert_eq!(run.limits.ceilings().1, CEILING, "minute {minute}");
        }
        assert!(run.levels.iter().skip(1).all(|&l| l == CEILING));
        assert_eq!(run.offs, 0);
    }

    #[test]
    fn a_garbage_temperature_alone_never_turns_it_off() {
        let mut run = Run::new();
        run.ui = CEILING;
        run.adc.temperature = 0;
        assert_eq!(run.secs(600), FLOOR);
        assert_eq!(run.offs, 0);
        assert_eq!(run.limits.health(), Health::Good);
    }

    #[test]
    fn no_result_at_all_is_garbage_too() {
        let mut limits = Limits::new(CALIBRATION, Faults::NONE);
        let off = (0..5 * TICKS_PER_SECOND).position(|_| limits.tick(CEILING, None).off);
        assert!(off.is_some());
        assert_eq!(limits.health(), Health::Fault);
        assert_eq!(limits.ceilings().1, FLOOR);
    }

    #[test]
    fn the_temperature_is_only_read_while_on() {
        let mut run = Run::new();
        let reads = |run: &Run| {
            run.limits.convert().map(|c| c.channel) == Some(crate::sensors::Channel::Temperature)
        };
        for _ in 0..60 * TICKS_PER_SECOND {
            run.tick();
            assert!(!reads(&run), "off");
        }
        run.ui = 50;
        let mut temperatures = 0;
        for _ in 0..TICKS_PER_SECOND {
            run.tick();
            temperatures += u32::from(reads(&run));
        }
        assert!(temperatures > 0, "on");
    }

    #[test]
    fn turning_on_resets_thermal_regulation() {
        let mut run = Run::new();
        run.ui = CEILING;
        run.adc.temperature = celsius(70);
        run.secs(60);
        assert_eq!(run.limits.ceilings().1, FLOOR);
        run.ui = 0;
        run.adc.temperature = celsius(25);
        run.secs(1);
        run.ui = CEILING;
        assert_eq!(run.ticks(1), CEILING);
    }

    #[test]
    fn a_rest_starts_no_conversions_and_stays_dark() {
        let mut run = Run::new();
        run.ticks(1);
        assert!(run.limits.convert().is_some());
        run.limits.rest();
        assert_eq!(run.limits.convert(), None);
        assert_eq!(run.limits.level(), 0);
    }

    #[test]
    fn heat_doesnt_carry_over_a_rest() {
        let mut run = Run::new();
        run.ui = CEILING;
        run.adc.temperature = celsius(70);
        run.secs(120);
        assert_eq!(run.limits.ceilings().1, FLOOR);
        run.ui = 0;
        run.ticks(1);
        run.limits.rest();
        run.adc.temperature = celsius(25);
        run.ui = CEILING;
        assert_eq!(run.ticks(1), CEILING);
    }

    #[test]
    fn a_rest_keeps_the_battery_ceiling_until_the_cell_proves_itself() {
        let mut run = Run::new();
        run.ui = CEILING;
        run.adc.voltage = voltage_reading(295);
        run.secs(6);
        let ceiling = run.limits.ceilings().0;
        assert!(ceiling < CEILING);
        run.ui = 0;
        run.ticks(1);
        run.limits.rest();
        run.adc.voltage = voltage_reading(380);
        assert_eq!(run.limits.ceilings().0, ceiling);
        run.ui = CEILING;
        assert_eq!(run.ticks(1), ceiling);
        run.secs(30);
        assert_eq!(run.limits.ceilings().0, CEILING);
    }

    #[test]
    fn a_rest_keeps_a_fault_and_a_flat_cell() {
        let mut run = Run::with(FAILED);
        run.limits.rest();
        assert_eq!(run.limits.health(), Health::Fault);
        let mut run = Run::new();
        run.adc.voltage = voltage_reading(280);
        run.ui = 50;
        run.secs(6);
        for _ in 0..battery::FORGET_TICKS {
            run.limits.rest();
        }
        assert_eq!(run.limits.health(), Health::Flat);
    }

    #[test]
    fn an_unloaded_reading_is_one_with_the_light_out_for_two_ticks_before_it() {
        let due = |tick: usize| tick % usize::from(CYCLE_TICKS) == 2;
        for voltage in [voltage_reading(320), 0] {
            let mut limits = Limits::new(CALIBRATION, Faults::NONE);
            let mut adc = Adc::new();
            adc.voltage = voltage;
            for tick in 0..64 {
                let step = limits.tick(0, adc.result());
                adc.start(limits.convert());
                let read = due(tick).then(|| battery::centivolts(voltage));
                assert_eq!(step.unloaded, read, "{voltage}: tick {tick}");
            }
        }
        for out_at in 64..80 {
            let mut limits = Limits::new(CALIBRATION, Faults::NONE);
            let mut adc = Adc::new();
            for tick in 0..out_at + 40 {
                let ui = if tick < out_at { 50 } else { 0 };
                let step = limits.tick(ui, adc.result());
                adc.start(limits.convert());
                let read = (due(tick) && tick >= out_at + 2).then_some(380);
                assert_eq!(step.unloaded, read, "out at {out_at}: tick {tick}");
            }
        }
    }

    #[test]
    fn a_check_at_rest_seeds_the_filter_afresh_and_leaves_the_ceiling() {
        use crate::torch::CHECK_TICKS;
        assert_eq!(CHECK_TICKS, battery::FORGET_TICKS);
        let mut run = Run::new();
        run.ui = CEILING;
        run.adc.voltage = voltage_reading(295);
        run.secs(6);
        let ceiling = run.limits.ceilings().0;
        assert!(ceiling < CEILING);
        run.ui = 0;
        run.ticks(1);
        run.limits.rest();
        for cv in [380, 330, 400] {
            for _ in 1..CHECK_TICKS {
                run.limits.rest();
            }
            assert_eq!(run.limits.protection.centivolts(), None, "{cv}");
            run.adc.voltage = voltage_reading(cv);
            run.ticks(3);
            run.limits.rest();
            assert_eq!(run.limits.protection.centivolts(), Some(cv));
            assert_eq!(run.limits.ceilings().0, ceiling, "{cv}");
            assert_eq!(run.limits.health(), Health::Good, "{cv}");
        }
    }

    #[test]
    fn garbage_from_power_on_shows_nothing_from_off() {
        let (voltage, temperature) = (Adc::new().voltage, Adc::new().temperature);
        for (voltage, temperature) in [
            (0, temperature),
            (voltage, 0),
            (0, 0),
            (FULL_SCALE, FULL_SCALE),
        ] {
            let mut run = Run::new();
            run.adc.voltage = voltage;
            run.adc.temperature = temperature;
            run.secs(10);
            assert!(
                run.levels.iter().all(|&l| l == 0),
                "{voltage}, {temperature}"
            );
            assert_eq!(run.offs, 0);
        }
    }

    #[test]
    fn a_failed_self_test_never_lights() {
        let mut run = Run::with(FAILED);
        assert_eq!(run.limits.health(), Health::Fault);
        assert_eq!(run.secs(10), 0);
        for ui in [1, 30, CEILING, LEVELS] {
            run.ui = ui;
            assert_eq!(run.secs(10), 0, "{ui}");
        }
        assert!(run.levels.iter().all(|&l| l == 0));
        assert_eq!(run.offs, 4, "asked for off every time");
    }

    fn throw_noise_at(mut limits: Limits, cap: u8, seed: u32) {
        let mut rng = crate::testing::Rng::new(seed);
        let mut ticks = 0_u32;
        while ticks < 1_000_000 {
            let ui = match rng.next_u16() % 8 {
                0 => 0,
                1 => 255,
                _ => rng.next_u16() as u8,
            };
            let run = 1 + u32::from(rng.next_u16() % 4000);
            let mode = rng.next_u16() % 3;
            let held = rng.next_u16() % (FULL_SCALE + 1);
            for _ in 0..run {
                let result = match (rng.next_u16() % 4, mode) {
                    (0, _) => None,
                    (_, 0) => Some(rng.next_u16()),
                    (_, 1) => Some(rng.next_u16() % (FULL_SCALE + 1)),
                    _ => Some(held),
                };
                let step = limits.tick(ui, result);
                assert!(
                    step.level <= ui.min(cap),
                    "ui {ui}, result {result:?}: level {}",
                    step.level
                );
                assert!(step.level <= CEILING, "level {}", step.level);
                assert!(!step.off || step.level == 0, "off at level {}", step.level);
                assert_eq!(limits.level(), step.level);
            }
            ticks += run;
        }
    }

    #[test]
    fn random_readings_never_lift_the_level_over_the_ui_or_the_cap() {
        throw_noise_at(Limits::new(CALIBRATION, Faults::NONE), CEILING, 0x5EED_0001);
    }

    #[test]
    fn random_readings_never_light_a_failed_self_test() {
        throw_noise_at(Limits::new(CALIBRATION, FAILED), 0, 0x5EED_0002);
    }
}
