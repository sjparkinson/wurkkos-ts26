//! One pass of the firmware's loop, minus the hardware: a switch sample and
//! an ADC result in, a level and the next conversion out.

use crate::button::{Clicks, Event};
use crate::limits::Limits;
use crate::ramp::{self, Output};
use crate::selftest::Faults;
use crate::sensors::Convert;
use crate::temp::Calibration;
use crate::ui::{Aux, Ui};

#[derive(Clone, Debug)]
pub struct Torch {
    clicks: Clicks,
    ui: Ui,
    limits: Limits,
}

impl Torch {
    /// `calibration` is the temperature sensor's, from the signature row, and
    /// `faults` what the power-on self-tests found.
    pub const fn new(calibration: Calibration, faults: Faults) -> Self {
        Self {
            clicks: Clicks::new(),
            ui: Ui::new(),
            limits: Limits::new(calibration, faults),
        }
    }

    /// One 64 Hz tick, `pressed` meaning the switch is down and `adc` the
    /// ADC's result, if it has one ready. Returns the click or hold this
    /// sample finished, if any.
    pub fn tick(&mut self, pressed: bool, adc: Option<u16>) -> Option<Event> {
        let event = self.clicks.tick(pressed);
        if let Some(event) = event {
            self.ui.handle(event);
        }
        if !self.resting() && self.limits.tick(self.ui.output_level(), adc).off {
            self.ui.off();
        }
        // Checked again, so a pass that turns the light off has already
        // rested if the firmware powers down straight after it.
        if self.resting() {
            self.limits.rest();
        }
        event
    }

    /// Whether there's nothing to do until the switch moves: the light's off
    /// and no click is being counted. The firmware powers down between these
    /// ticks, and a press ends it on the tick that sees the press, in time to
    /// read the cell before a click can turn the light on.
    pub fn resting(&self) -> bool {
        self.clicks.idle() && self.ui.output_level() == 0 && self.limits.level() == 0
    }

    /// The level the LEDs should be at right now, `0` meaning off: what the
    /// UI wants, as far as the cell and the temperature allow.
    pub fn level(&self) -> u8 {
        self.limits.level()
    }

    /// The conversion to start on the ADC after this tick, if any.
    pub fn convert(&self) -> Option<Convert> {
        self.limits.convert()
    }

    pub fn output(&self) -> Output {
        ramp::output(self.level())
    }

    pub fn aux(&self) -> Aux {
        self.ui.aux()
    }
}

/// The last value sent to the hardware, so the loop only rewrites the
/// timer and the enable pin when the output moves.
#[derive(Clone, Copy, Debug, Default)]
pub struct Changes<T> {
    last: Option<T>,
}

impl<T: Copy + PartialEq> Changes<T> {
    pub const fn new() -> Self {
        Self { last: None }
    }

    /// `Some(value)` the first time and whenever `value` differs from the
    /// last one seen, `None` otherwise.
    pub fn update(&mut self, value: T) -> Option<T> {
        if self.last == Some(value) {
            return None;
        }
        self.last = Some(value);
        Some(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TICKS_PER_SECOND;
    use crate::button::RELEASE_TICKS;
    use crate::limits::FAULT_CEILING;
    use crate::testing::{Adc, CALIBRATION, temperature_reading, voltage_reading};
    use crate::thermal;
    use crate::ui::{CEIL, DEFAULT_LEVEL, FLOOR};
    use core::ops::Deref;

    const UI_MD: &str = include_str!("../docs/ui.md");

    /// A torch wired to a fake ADC, the way `src/main.rs` wires it to the real
    /// one: a good cell at room temperature unless a test says otherwise.
    struct Bench {
        torch: Torch,
        adc: Adc,
    }

    impl Bench {
        fn new() -> Self {
            Self::with(Faults::NONE)
        }

        fn with(faults: Faults) -> Self {
            Self {
                torch: Torch::new(CALIBRATION, faults),
                adc: Adc::new(),
            }
        }

        fn tick(&mut self, pressed: bool) -> Option<Event> {
            let event = self.torch.tick(pressed, self.adc.result());
            self.adc.start(self.torch.convert());
            event
        }
    }

    impl Deref for Bench {
        type Target = Torch;

        fn deref(&self) -> &Torch {
            &self.torch
        }
    }

    fn dim() -> Output {
        ramp::output(DEFAULT_LEVEL)
    }

    fn run(torch: &mut Bench, pressed: bool, ticks: u16) {
        for _ in 0..ticks {
            torch.tick(pressed);
        }
    }

    fn clicks(torch: &mut Bench, n: u8) {
        for _ in 0..n {
            run(torch, true, 4);
            run(torch, false, 4);
        }
    }

    fn click(torch: &mut Bench) {
        clicks(torch, 1);
        run(torch, false, RELEASE_TICKS + 1);
    }

    /// `2s` or `500ms` in ticks.
    fn duration(text: &str) -> u16 {
        let number = |digits: &str| digits.parse::<u16>().unwrap();
        match text.strip_suffix("ms") {
            Some(ms) => number(ms) * TICKS_PER_SECOND / 1000,
            None => number(text.strip_suffix('s').unwrap()) * TICKS_PER_SECOND,
        }
    }

    /// Plays a docs/ui.md script with the timings `cargo xtask simulate` uses,
    /// so these rows hold there too, and returns the level it ends at.
    fn play(script: &str) -> u8 {
        let mut torch = Bench::new();
        for word in script.split_whitespace() {
            if word == "fault" {
                torch = Bench::with(Faults {
                    fuses: true,
                    ..Faults::NONE
                });
                continue;
            }
            if let Some(volts) = word.strip_prefix("cell:") {
                let centivolts = volts.strip_suffix('V').unwrap().replace('.', "");
                torch.adc.voltage = voltage_reading(centivolts.parse().unwrap());
                continue;
            }
            if let Some(degrees) = word.strip_prefix("heat:") {
                let degrees = degrees.strip_suffix('C').unwrap().parse().unwrap();
                torch.adc.temperature = temperature_reading(thermal::celsius(degrees));
                continue;
            }
            if let Some(time) = word.strip_prefix("wait:") {
                run(&mut torch, false, duration(time));
                continue;
            }
            let (word, time) = word.split_once(':').unwrap_or((word, "1s"));
            let (count, kind) = word.split_at(word.len() - 1);
            let count: u8 = count.parse().unwrap();
            match kind {
                "C" => clicks(&mut torch, count),
                "H" => {
                    clicks(&mut torch, count - 1);
                    run(&mut torch, true, duration(time));
                }
                _ => panic!("can't read {word}"),
            }
            run(&mut torch, false, RELEASE_TICKS + 1);
        }
        torch.level()
    }

    #[test]
    fn every_example_in_docs_ui_md_holds() {
        let mut checked = 0;
        for row in UI_MD.lines().filter(|line| line.starts_with('|')) {
            let Some(example) = row.split('|').find(|cell| cell.contains('→')) else {
                continue;
            };
            let (script, expect) = example.split_once('→').unwrap();
            let level = play(script.trim().trim_matches('`'));
            match expect.trim() {
                "off" => assert_eq!(level, 0, "{row}"),
                "on" => assert!(level > 0, "{row}"),
                other => {
                    let want = other.strip_prefix("level ").unwrap();
                    assert_eq!(level, want.parse::<u8>().unwrap(), "{row}");
                }
            }
            checked += 1;
        }
        assert!(checked >= 22, "only found {checked} examples");
    }

    #[test]
    fn starts_off() {
        let mut torch = Bench::new();
        assert_eq!(torch.tick(false), None);
        assert_eq!(torch.level(), 0);
        assert_eq!(torch.output(), Output::OFF);
    }

    #[test]
    fn a_click_turns_it_on_once_the_click_counts() {
        let mut torch = Bench::new();
        run(&mut torch, true, 4);
        run(&mut torch, false, RELEASE_TICKS);
        assert_eq!(torch.output(), Output::OFF);
        assert_eq!(torch.tick(false), Some(Event::Click(1)));
        assert_eq!(torch.output(), dim());
    }

    #[test]
    fn a_second_click_turns_it_off() {
        let mut torch = Bench::new();
        click(&mut torch);
        click(&mut torch);
        assert_eq!(torch.output(), Output::OFF);
    }

    /// Lockout: 1H. A script can only see the light after the hold ends.
    #[test]
    fn lockout_lights_at_the_floor_only_while_held() {
        let mut torch = Bench::new();
        clicks(&mut torch, 4);
        run(&mut torch, false, RELEASE_TICKS + 1);
        run(&mut torch, true, 2 * TICKS_PER_SECOND);
        assert_eq!(torch.level(), FLOOR);
        run(&mut torch, false, 1);
        assert_eq!(torch.level(), 0);
    }

    #[test]
    fn the_ramp_never_asks_for_more_than_the_ceiling() {
        let mut torch = Bench::new();
        click(&mut torch);
        run(&mut torch, true, 10 * TICKS_PER_SECOND);
        assert_eq!(torch.level(), CEIL);
        assert_eq!(torch.output(), ramp::output(ramp::CEILING));
    }

    #[test]
    fn rests_only_with_the_light_off_and_no_click_counting() {
        let mut torch = Bench::new();
        assert!(torch.resting());
        run(&mut torch, true, 4);
        assert!(!torch.resting(), "pressed");
        run(&mut torch, false, RELEASE_TICKS);
        assert!(!torch.resting(), "a click waiting to count");
        run(&mut torch, false, 1);
        assert!(!torch.resting(), "on");
        click(&mut torch);
        assert!(torch.resting());
    }

    #[test]
    fn a_locked_torch_rests() {
        let mut torch = Bench::new();
        clicks(&mut torch, 4);
        run(&mut torch, false, RELEASE_TICKS + 1);
        assert!(torch.resting());
    }

    #[test]
    fn the_aux_leds_follow_the_ui() {
        let mut torch = Bench::new();
        assert_eq!(torch.aux(), Aux::Blue);
        click(&mut torch);
        assert_eq!(torch.aux(), Aux::Off);
    }

    #[test]
    fn a_flat_cell_turns_the_ui_off_and_it_keeps_the_level() {
        let mut torch = Bench::new();
        click(&mut torch);
        torch.adc.voltage = voltage_reading(280);
        run(&mut torch, false, 10 * TICKS_PER_SECOND);
        assert_eq!(torch.level(), 0);
        assert_eq!(torch.ui.output_level(), 0, "the UI's off too");
        // Not left with the UI still on and the light out.
        assert!(torch.resting());
        torch.adc.voltage = voltage_reading(380);
        click(&mut torch);
        assert_eq!(torch.ui.output_level(), DEFAULT_LEVEL);
    }

    #[test]
    fn the_pass_that_turns_the_light_off_has_already_rested() {
        let mut torch = Bench::new();
        click(&mut torch);
        torch.adc.voltage = voltage_reading(280);
        while torch.level() != 0 {
            torch.tick(false);
        }
        assert!(torch.resting());
        assert_eq!(
            torch.convert(),
            None,
            "nothing's left to read in power-down"
        );
    }

    #[test]
    fn heat_lowers_the_light_and_leaves_the_ui_where_it_was() {
        let mut torch = Bench::new();
        clicks(&mut torch, 2);
        run(&mut torch, false, RELEASE_TICKS + 1);
        torch.adc.temperature = temperature_reading(thermal::celsius(80));
        run(&mut torch, false, 120 * TICKS_PER_SECOND);
        assert_eq!(torch.level(), thermal::Config::DEFAULT.floor);
        assert_eq!(torch.output(), ramp::output(torch.level()));
        assert_eq!(torch.ui.output_level(), CEIL);
    }

    #[test]
    fn a_fault_caps_the_light_and_only_the_light() {
        for faults in [
            Faults {
                flash: true,
                ..Faults::NONE
            },
            Faults {
                fuses: true,
                ..Faults::NONE
            },
            Faults {
                hang: true,
                ..Faults::NONE
            },
        ] {
            let mut torch = Bench::with(faults);
            clicks(&mut torch, 2);
            run(&mut torch, false, RELEASE_TICKS + 1);
            assert_eq!(torch.level(), FAULT_CEILING);
            assert_eq!(torch.ui.output_level(), CEIL);
        }
    }

    #[test]
    fn a_resting_torch_reads_nothing_and_a_press_starts_the_adc() {
        let mut torch = Bench::new();
        run(&mut torch, false, 64);
        assert!(torch.resting());
        assert_eq!(torch.convert(), None);
        torch.tick(true);
        assert!(!torch.resting());
        assert!(torch.convert().is_some_and(|convert| convert.setup));
    }

    #[test]
    fn still_turns_off_after_hours_on() {
        let mut torch = Bench::new();
        click(&mut torch);
        // Past where button::Clicks' tick count saturates.
        for _ in 0..4 {
            run(&mut torch, false, u16::MAX);
        }
        assert_eq!(torch.output(), dim());
        click(&mut torch);
        assert_eq!(torch.output(), Output::OFF);
    }

    #[test]
    fn the_first_output_always_counts_as_a_change() {
        assert_eq!(Changes::new().update(Output::OFF), Some(Output::OFF));
        assert_eq!(Changes::new().update(dim()), Some(dim()));
    }

    #[test]
    fn turning_on_and_off_are_changes_and_staying_put_is_not() {
        let mut changes = Changes::new();
        assert_eq!(changes.update(Output::OFF), Some(Output::OFF));
        assert_eq!(changes.update(Output::OFF), None);
        assert_eq!(changes.update(dim()), Some(dim()));
        assert_eq!(changes.update(dim()), None);
        assert_eq!(changes.update(Output::OFF), Some(Output::OFF));
    }
}
