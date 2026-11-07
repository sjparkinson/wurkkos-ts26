//! What the cell and the temperature allow: the UI's level in, the level to
//! run at out.
//!
//! Each tick this runs the ADC's [`Schedule`], hands each reading to
//! low-voltage protection (`battery::Protection`) or thermal regulation
//! (`thermal::Thermal`), and runs the light at the lowest of the UI's level,
//! the thermal ceiling and the battery ceiling. Both ceilings fade through a
//! [`Gradual`], so a step-down never jumps. A flat cell turns the UI off.
//!
//! Garbage readings fail safe. Each side has its own idea of a reading that
//! can't be real (`battery::plausible`, `thermal::plausible`), lets a few
//! go, and then drops its ceiling rather than trusting it or turning the
//! light off: the battery side to [`FAULT_CEILING`], the thermal side to its
//! floor.
//!
//! A failed power-on self-test caps the light at [`FAULT_CEILING`] for as
//! long as the cell's in.

use crate::battery::{self, Protection, Verdict};
use crate::gradual::Gradual;
use crate::ramp::CEILING;
use crate::selftest::Faults;
use crate::sensors::{Convert, Reading, Schedule};
use crate::temp::Calibration;
use crate::thermal::{self, Thermal};

/// The highest level the torch runs at when something's wrong that it can't
/// see past: a failed self-test, or a voltage reading it can't believe. The
/// TS21 fell back on its 350 mA linear channel. This board has nothing like
/// that, so this borrows the thermal floor's level, a few per cent of full
/// duty on our curve. Unmeasured: bright enough to find your way out by,
/// gentle on the cell as far as anyone can say without a meter on it.
pub const FAULT_CEILING: u8 = thermal::FLOOR;

const _: () = assert!(FAULT_CEILING <= CEILING);

/// Ticks per level as the battery ceiling fades down, and back up. Down is a
/// level a tick, quicker than thermal's, since the cell's already under its
/// threshold by the time it steps: 15 levels takes a quarter of a second,
/// and a drop of 30 under half. Unmeasured.
const BATTERY_DOWN_EVERY: u8 = 1;
const BATTERY_UP_EVERY: u8 = 4;

/// One tick's answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Step {
    /// The level to run at, `0` meaning off.
    pub level: u8,
    /// The cell's flat: turn the UI off.
    pub off: bool,
}

#[derive(Clone, Debug)]
pub struct Limits {
    schedule: Schedule,
    calibration: Calibration,
    protection: Protection,
    /// The battery ceiling and where it's fading to.
    battery: Gradual,
    battery_target: u8,
    thermal: Thermal,
    /// Whether the UI was on last tick.
    on: bool,
    /// The level the light's at, as of the last tick.
    level: u8,
    convert: Option<Convert>,
    /// The highest level the light may ever run at: `ramp::CEILING`, or
    /// [`FAULT_CEILING`] if a self-test failed.
    cap: u8,
}

impl Limits {
    /// `calibration` is the chip's own, from the signature row. `faults` is
    /// what the power-on self-tests found, and any fault holds the light at
    /// [`FAULT_CEILING`] for as long as this lives.
    pub const fn new(calibration: Calibration, faults: Faults) -> Self {
        Self {
            schedule: Schedule::new(),
            calibration,
            protection: Protection::new(),
            battery: Gradual::new(CEILING, BATTERY_DOWN_EVERY, BATTERY_UP_EVERY),
            battery_target: CEILING,
            thermal: Thermal::new(),
            on: false,
            level: 0,
            convert: None,
            cap: if faults.any() { FAULT_CEILING } else { CEILING },
        }
    }

    /// The conversion the ADC should start this tick, if any.
    pub fn convert(&self) -> Option<Convert> {
        self.convert
    }

    /// The level the light's at, `0` meaning off: the last tick's
    /// [`Step::level`].
    pub fn level(&self) -> u8 {
        self.level
    }

    /// The battery and thermal ceilings right now.
    pub fn ceilings(&self) -> (u8, u8) {
        (self.battery.level(), self.thermal.ceiling())
    }

    /// A tick in place of [`Limits::tick`] while the torch rests: the light's
    /// off, and nothing's measured, since the ADC can't run in power-down. A
    /// conversion started now would come back on waking with an invalid
    /// result (DS40002204A §30.3.5). So a battery ceiling only climbs once
    /// readings start again, and a long rest forgets the last ones
    /// (`battery::FORGET_TICKS`).
    pub fn rest(&mut self) {
        self.schedule.rest();
        self.protection.rest();
        self.convert = None;
        self.on = false;
        self.level = 0;
    }

    /// One tick: `ui` is the level the UI wants (`0` for off) and `result`
    /// the ADC's result, if it has one ready.
    pub fn tick(&mut self, ui: u8, result: Option<u16>) -> Step {
        let on = ui != 0;
        if on && !self.on {
            self.thermal.reset();
        }
        self.on = on;
        // The level the light's at this tick, before the ceilings move. It's
        // last tick's level while the UI holds steady, and the UI's on the
        // tick it turns on.
        let ui = ui.min(self.cap);
        let level = match on {
            true => ui.min(self.thermal.ceiling()).min(self.battery.level()),
            false => 0,
        };

        let (reading, convert) = self.schedule.tick(result, on);
        self.convert = convert;
        let mut off = false;
        match reading {
            Some(Reading::Voltage(raw)) => {
                let centivolts = battery::centivolts(raw);
                match self.protection.sample(centivolts, level) {
                    Verdict::Off => off = true,
                    Verdict::Ceiling(ceiling) => self.battery_ceiling(ceiling, level),
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

        let thermal = self.thermal.tick();
        let battery = self.battery.tick(self.battery_target);
        self.level = if on && !off {
            ui.min(thermal).min(battery)
        } else {
            0
        };
        Step {
            level: self.level,
            off,
        }
    }

    /// A new battery ceiling, with the light at `level`. Like thermal's, a
    /// ceiling above the light's level does nothing, so it's pulled down to
    /// the level first, out of sight, and the fade shows from its first
    /// step.
    fn battery_ceiling(&mut self, ceiling: u8, level: u8) {
        if ceiling < self.battery.level() {
            self.battery.cap(level.max(ceiling));
        }
        self.battery_target = ceiling;
    }
}

#[cfg(test)]
mod tests {
    extern crate std;

    use super::*;
    use crate::TICKS_PER_SECOND;
    use crate::ramp::LEVELS;
    use crate::sensors::CYCLE_TICKS;
    use crate::testing::{Adc, CALIBRATION, temperature_reading, voltage_reading};
    use crate::thermal::{self, FLOOR};
    use std::vec::Vec;

    /// Self-tests that found something.
    const FAILED: Faults = Faults {
        hang: true,
        ..Faults::NONE
    };

    /// Limits and a fake ADC, a tick at a time, checking the output never
    /// jumps by more than a level while the UI holds steady.
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

        /// After power-on self-tests that found `faults`.
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
            // Only a change of UI level (or the cell going flat) may jump.
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

    /// The raw temperature reading for `degrees` Celsius.
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
        // From 50 to 35 took a level a tick, not one jump.
        for level in 35..50 {
            assert!(run.levels.contains(&level), "{level}");
        }
        run.secs(30);
        assert_eq!(run.offs, 1);
        assert_eq!(*run.levels.last().unwrap(), 0);
    }

    #[test]
    fn a_flat_cell_fades_down_the_ramp_before_it_turns_off() {
        let mut run = Run::new();
        run.ui = CEILING;
        run.secs(1);
        run.adc.voltage = voltage_reading(280);
        // The filter takes a few seconds to fall under 2.9 V, then the hold
        // a second, so it's still at the top.
        assert_eq!(run.secs(4), CEILING);
        assert_eq!(run.offs, 0);
        // Then it fades all the way down, without ever jumping, and with the
        // cell still flat it turns off once it's had a second at level 1.
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
        // Voltage readings come due on the third tick of each round of 16.
        let sample = usize::from(CYCLE_TICKS);
        let mut run = Run::new();
        run.adc.voltage = voltage_reading(280);
        let on_at = 4 * sample + 2;
        run.ticks(on_at as u32);
        run.ui = 1;
        run.ticks(1);
        assert_eq!(run.levels[on_at], 1);
        run.secs(2);
        // A second of flat samples, the first on the turn-on tick itself.
        let off_at = on_at + (usize::from(battery::HOLD_SAMPLES) - 1) * sample;
        assert!(run.levels[on_at..off_at].iter().all(|&l| l == 1));
        assert_eq!(run.levels[off_at], 0);
        assert_eq!(run.offs, 1);
    }

    #[test]
    fn a_flat_cell_turns_off_again_after_turning_back_on() {
        let mut run = Run::new();
        run.adc.voltage = voltage_reading(280);
        run.ui = 50;
        run.secs(6);
        assert_eq!(run.offs, 1);
        run.ui = 50;
        // Straight to where it left off, and off again a second later.
        assert_eq!(run.ticks(16), 1);
        run.secs(2);
        assert_eq!(run.offs, 2);
    }

    #[test]
    fn garbage_voltages_cap_it_at_the_fault_ceiling_and_never_turn_it_off() {
        for garbage in [0, 4 * 1023, 1] {
            let mut run = Run::new();
            run.ui = CEILING;
            run.secs(2);
            run.adc.voltage = garbage;
            run.secs(1);
            assert_eq!(run.secs(10), FAULT_CEILING, "{garbage}");
            assert_eq!(run.secs(600), FAULT_CEILING, "{garbage}");
            assert_eq!(run.offs, 0);
        }
    }

    #[test]
    fn a_dead_adc_brings_both_ceilings_down() {
        let mut run = Run::new();
        run.ui = CEILING;
        run.secs(1);
        run.adc.voltage = 0;
        run.adc.temperature = 0;
        assert_eq!(run.secs(30), FAULT_CEILING);
        assert_eq!(run.limits.ceilings(), (FAULT_CEILING, FLOOR));
        assert_eq!(run.offs, 0);
    }

    #[test]
    fn no_result_at_all_is_garbage_too() {
        let mut limits = Limits::new(CALIBRATION, Faults::NONE);
        for _ in 0..30 * TICKS_PER_SECOND {
            limits.tick(CEILING, None);
        }
        assert_eq!(limits.ceilings(), (FAULT_CEILING, FLOOR));
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
        // Straight back to the top: last time's heat doesn't carry over.
        assert_eq!(run.ticks(1), CEILING);
    }

    #[test]
    fn a_rest_starts_no_conversions_and_stays_dark() {
        let mut run = Run::new();
        // A tick of the round that starts a conversion.
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
    fn garbage_from_power_on_shows_nothing_from_off() {
        let (voltage, temperature) = (Adc::new().voltage, Adc::new().temperature);
        for (voltage, temperature) in [(0, temperature), (voltage, 0), (0, 0), (4 * 1023, 4 * 1023)]
        {
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
    fn a_failed_self_test_holds_the_fault_ceiling_for_good() {
        let mut run = Run::with(FAILED);
        run.secs(3);
        run.ui = LEVELS;
        assert_eq!(run.secs(600), FAULT_CEILING);
        assert!(run.levels.iter().all(|&l| l <= FAULT_CEILING));
        // Off and on again doesn't clear it. Only a power cycle with a good
        // self-test does.
        run.ui = 0;
        run.secs(1);
        run.ui = LEVELS;
        assert_eq!(run.ticks(1), FAULT_CEILING);
        assert_eq!(run.offs, 0);
    }

    #[test]
    fn a_failed_self_test_leaves_lower_levels_alone() {
        let mut run = Run::with(FAILED);
        run.secs(3);
        for ui in [1, 30, FAULT_CEILING] {
            run.ui = ui;
            assert_eq!(run.secs(10), ui);
        }
    }

    #[test]
    fn a_failed_self_test_still_lets_the_battery_turn_it_off() {
        let mut run = Run::with(FAILED);
        run.adc.voltage = voltage_reading(280);
        run.ui = LEVELS;
        run.secs(10);
        assert_eq!(run.offs, 1);
        assert!(run.levels.iter().all(|&l| l <= FAULT_CEILING));
    }

    #[test]
    fn a_failed_self_test_shows_nothing_from_off() {
        let mut run = Run::with(FAILED);
        assert_eq!(run.secs(10), 0);
        assert!(run.levels.iter().all(|&l| l == 0));
    }
}
