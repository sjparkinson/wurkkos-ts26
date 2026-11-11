//! One pass of the firmware's loop, minus the hardware. A switch sample and
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

    /// Whether there's nothing to do until the switch moves, meaning the
    /// light's off and no click is being counted. The firmware powers down
    /// between these ticks, and a press ends it on the tick that sees the
    /// press, in time to read the cell before a click can turn the light on.
    pub fn resting(&self) -> bool {
        self.clicks.idle() && self.ui.output_level() == 0 && self.limits.level() == 0
    }

    /// The level the LEDs should be at right now, `0` meaning off. That's what
    /// the UI wants, as far as the cell and the temperature allow.
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
    use crate::ramp::CEILING;
    use crate::testing::{Rig, examples, temperature_reading, voltage_reading};
    use crate::thermal;
    use crate::ui::{DEFAULT_LEVEL, FLOOR};

    const UI_MD: &str = include_str!("../docs/ui.md");

    fn dim() -> Output {
        ramp::output(DEFAULT_LEVEL)
    }

    fn run(torch: &mut Rig, pressed: bool, ticks: u16) {
        for _ in 0..ticks {
            torch.tick(pressed);
        }
    }

    fn clicks(torch: &mut Rig, n: u8) {
        for _ in 0..n {
            run(torch, true, 4);
            run(torch, false, 4);
        }
    }

    fn click(torch: &mut Rig) {
        clicks(torch, 1);
        run(torch, false, RELEASE_TICKS + 1);
    }

    /// Every row's example, played the way `cargo xtask simulate` and the
    /// firmware's run in `cargo xtask hex` play it.
    #[test]
    fn every_example_in_docs_ui_md_holds() {
        let mut found = false;
        for example in examples(UI_MD) {
            let example = example.unwrap_or_else(|e| panic!("docs/ui.md:{}: {e}", e.line));
            let level = Rig::play(example.actions()).level();
            assert!(
                example.expect.holds(level),
                "docs/ui.md:{}: {}: expected {:?}, but it ends at level {level}",
                example.line,
                example.row,
                example.expect
            );
            found = true;
        }
        assert!(found, "docs/ui.md has no examples");
    }

    #[test]
    fn starts_off() {
        let mut torch = Rig::default();
        assert_eq!(torch.tick(false), None);
        assert_eq!(torch.level(), 0);
        assert_eq!(torch.output(), Output::OFF);
    }

    #[test]
    fn a_click_turns_it_on_once_the_click_counts() {
        let mut torch = Rig::default();
        run(&mut torch, true, 4);
        run(&mut torch, false, RELEASE_TICKS);
        assert_eq!(torch.output(), Output::OFF);
        assert_eq!(torch.tick(false), Some(Event::Click(1)));
        assert_eq!(torch.output(), dim());
    }

    #[test]
    fn a_second_click_turns_it_off() {
        let mut torch = Rig::default();
        click(&mut torch);
        click(&mut torch);
        assert_eq!(torch.output(), Output::OFF);
    }

    /// Lockout (1H), to the tick it's let go.
    #[test]
    fn lockout_lights_at_the_floor_only_while_held() {
        let mut torch = Rig::default();
        clicks(&mut torch, 4);
        run(&mut torch, false, RELEASE_TICKS + 1);
        run(&mut torch, true, 2 * TICKS_PER_SECOND);
        assert_eq!(torch.level(), FLOOR);
        run(&mut torch, false, 1);
        assert_eq!(torch.level(), 0);
    }

    #[test]
    fn the_ramp_never_asks_for_more_than_the_ceiling() {
        let mut torch = Rig::default();
        click(&mut torch);
        run(&mut torch, true, 10 * TICKS_PER_SECOND);
        assert_eq!(torch.level(), CEILING);
        assert_eq!(torch.output(), ramp::output(ramp::CEILING));
    }

    #[test]
    fn rests_only_with_the_light_off_and_no_click_counting() {
        let mut torch = Rig::default();
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
        let mut torch = Rig::default();
        clicks(&mut torch, 4);
        run(&mut torch, false, RELEASE_TICKS + 1);
        assert!(torch.resting());
    }

    #[test]
    fn the_aux_leds_follow_the_ui() {
        let mut torch = Rig::default();
        assert_eq!(torch.aux(), Aux::Blue);
        click(&mut torch);
        assert_eq!(torch.aux(), Aux::Off);
    }

    #[test]
    fn a_flat_cell_turns_the_ui_off_and_it_keeps_the_level() {
        let mut torch = Rig::default();
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
        let mut torch = Rig::default();
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
    fn a_cell_it_cant_read_turns_the_ui_off_after_five_minutes_and_a_click_brings_it_back() {
        let mut torch = Rig::default();
        clicks(&mut torch, 2);
        run(&mut torch, false, RELEASE_TICKS + 1);
        torch.adc.voltage = 0;
        run(&mut torch, false, 10 * TICKS_PER_SECOND);
        assert_eq!(torch.level(), FAULT_CEILING);
        assert_eq!(torch.ui.output_level(), CEILING, "only the light's capped");
        // A second short of five minutes, then a second past.
        run(&mut torch, false, 289 * TICKS_PER_SECOND);
        assert_eq!(torch.level(), FAULT_CEILING);
        run(&mut torch, false, 2 * TICKS_PER_SECOND);
        assert_eq!(torch.level(), 0);
        assert_eq!(torch.ui.output_level(), 0, "the UI's off too");
        assert!(torch.resting());
        assert_eq!(torch.aux(), Aux::Blue, "off, not locked out");
        // It comes back at the level it was asked for, still capped, and
        // goes out again five minutes later.
        click(&mut torch);
        assert_eq!(torch.level(), FAULT_CEILING);
        assert_eq!(torch.ui.output_level(), CEILING);
        run(&mut torch, false, 299 * TICKS_PER_SECOND);
        assert_eq!(torch.level(), FAULT_CEILING);
        run(&mut torch, false, 2 * TICKS_PER_SECOND);
        assert!(torch.resting());
    }

    #[test]
    fn five_minutes_blind_puts_out_a_lockout_hold_and_leaves_it_locked() {
        let mut torch = Rig::default();
        torch.adc.voltage = 0;
        clicks(&mut torch, 4);
        run(&mut torch, false, RELEASE_TICKS + 1);
        run(&mut torch, true, 299 * TICKS_PER_SECOND);
        assert_eq!(torch.level(), FLOOR);
        run(&mut torch, true, 2 * TICKS_PER_SECOND);
        assert_eq!(torch.level(), 0, "out, with the switch still down");
        run(&mut torch, false, 1);
        assert_eq!(torch.aux(), Aux::Red);
        assert!(torch.resting());
    }

    #[test]
    fn heat_lowers_the_light_and_leaves_the_ui_where_it_was() {
        let mut torch = Rig::default();
        clicks(&mut torch, 2);
        run(&mut torch, false, RELEASE_TICKS + 1);
        torch.adc.temperature = temperature_reading(thermal::celsius(80));
        run(&mut torch, false, 120 * TICKS_PER_SECOND);
        assert_eq!(torch.level(), thermal::FLOOR);
        assert_eq!(torch.output(), ramp::output(torch.level()));
        assert_eq!(torch.ui.output_level(), CEILING);
    }

    /// A switch that's stuck down from the moment the battery goes in, say a
    /// jammed button. It's a hold from off, so it lights at the floor and
    /// never ramps.
    #[test]
    fn a_switch_held_from_power_on_stays_at_the_floor() {
        let mut torch = Rig::default();
        let mut highest = 0;
        for tick in 0..10 * 60 * u32::from(TICKS_PER_SECOND) {
            torch.tick(true);
            highest = highest.max(torch.level());
            if tick > 2 * u32::from(TICKS_PER_SECOND) {
                assert_eq!(torch.level(), FLOOR, "tick {tick}");
            }
        }
        assert_eq!(highest, FLOOR);
    }

    /// Heat doesn't care whether the switch is down. The light follows the
    /// thermal ceiling while the ramp's still being held.
    #[test]
    fn a_switch_held_while_on_still_answers_to_heat() {
        let mut torch = Rig::default();
        click(&mut torch);
        run(&mut torch, true, 10 * TICKS_PER_SECOND);
        assert_eq!(torch.level(), CEILING);
        torch.adc.temperature = temperature_reading(thermal::celsius(80));
        assert!(thermal::celsius(80) > thermal::LIMIT);
        for _ in 0..120 * TICKS_PER_SECOND {
            torch.tick(true);
        }
        assert_eq!(torch.level(), thermal::FLOOR);
        assert_eq!(torch.ui.output_level(), CEILING, "still held at the top");
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
            let mut torch = Rig::new(faults);
            clicks(&mut torch, 2);
            run(&mut torch, false, RELEASE_TICKS + 1);
            assert_eq!(torch.level(), FAULT_CEILING);
            assert_eq!(torch.ui.output_level(), CEILING);
        }
    }

    #[test]
    fn a_resting_torch_reads_nothing_and_a_press_starts_the_adc() {
        let mut torch = Rig::default();
        run(&mut torch, false, 64);
        assert!(torch.resting());
        assert_eq!(torch.convert(), None);
        torch.tick(true);
        assert!(!torch.resting());
        assert!(torch.convert().is_some_and(|convert| convert.setup));
    }

    #[test]
    fn still_turns_off_after_hours_on() {
        let mut torch = Rig::default();
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
