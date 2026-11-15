//! One pass of the firmware's loop, minus the hardware. A switch sample and
//! an ADC result in, a level and the next conversion out.

use crate::TICKS_PER_SECOND;
use crate::button::{Clicks, Event};
use crate::limits::{Health, Limits};
use crate::ramp::{self, Output};
use crate::selftest::Faults;
use crate::sensors::Convert;
use crate::temp::Calibration;
use crate::ui::{Aux, Ui};

/// How long the aux LEDs say why the light won't come on.
///
/// That's after a press, or after the light goes out. Three seconds, then
/// dark, so a flat cell with no protection of its own isn't drained by the
/// aux. Unmeasured.
pub const SHOW_TICKS: u16 = 3 * TICKS_PER_SECOND;

#[derive(Clone, Debug)]
pub struct Torch {
    clicks: Clicks,
    ui: Ui,
    limits: Limits,
    /// Ticks left of [`SHOW_TICKS`].
    showing: u16,
}

impl Torch {
    /// `calibration` is the temperature sensor's, from the signature row, and
    /// `faults` what the power-on self-tests found. A fault shows on the aux
    /// from power-on.
    pub const fn new(calibration: Calibration, faults: Faults) -> Self {
        Self {
            clicks: Clicks::new(),
            ui: Ui::new(),
            limits: Limits::new(calibration, faults),
            showing: if faults.any() { SHOW_TICKS } else { 0 },
        }
    }

    /// One 64 Hz tick, `pressed` meaning the switch is down and `adc` the
    /// ADC's result, if it has one ready. Returns the click or hold this
    /// sample finished, if any.
    ///
    /// With a flat cell or a fault, and the light out, clicks and holds never
    /// reach the UI. They show the aux colour instead, for [`SHOW_TICKS`]
    /// from the click or from the start of the hold, so a switch held down
    /// can't keep the aux lit. While a fault's fading the light out they
    /// still reach it, so 1C turns it off. A press reads the cell before its
    /// click counts, so a cell that's been charged is [`Health::Good`] by the
    /// time the click arrives, and that click turns the light on as usual.
    pub fn tick(&mut self, pressed: bool, adc: Option<u16>) -> Option<Event> {
        self.showing = self.showing.saturating_sub(1);
        let event = self.clicks.tick(pressed);
        if let Some(event) = event {
            if self.limits.health() == Health::Good || self.level() > 0 {
                self.ui.handle(event);
            } else if matches!(event, Event::Click(_) | Event::Hold { ticks: 0, .. }) {
                self.showing = SHOW_TICKS;
            }
        }
        let before = self.limits.health();
        if !self.resting() && self.limits.tick(self.ui.output_level(), adc).off {
            self.ui.off();
        }
        // Say why, from the moment it's decided, through any fade, and for a
        // while after the light's out.
        let health = self.limits.health();
        if health != Health::Good && (before == Health::Good || self.level() > 0) {
            self.showing = SHOW_TICKS;
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
    pub const fn resting(&self) -> bool {
        self.clicks.idle() && self.ui.output_level() == 0 && self.limits.level() == 0
    }

    /// The level the LEDs should be at right now, `0` meaning off. That's what
    /// the UI wants, as far as the cell and the temperature allow.
    pub const fn level(&self) -> u8 {
        self.limits.level()
    }

    /// The conversion to start on the ADC after this tick, if any.
    pub const fn convert(&self) -> Option<Convert> {
        self.limits.convert()
    }

    pub fn output(&self) -> Output {
        ramp::output(self.level())
    }

    /// The UI's colour while all's well. Otherwise yellow for a flat cell and
    /// white for a fault, for [`SHOW_TICKS`] after a press or after the light
    /// goes out, and dark the rest of the time.
    pub const fn aux(&self) -> Aux {
        match self.limits.health() {
            Health::Good => self.ui.aux(),
            _ if self.level() > 0 || self.showing == 0 => Aux::Off,
            Health::Flat => Aux::Yellow,
            Health::Fault => Aux::White,
        }
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
    use crate::button::RELEASE_TICKS;
    use crate::ramp::CEILING;
    use crate::testing::{Rig, examples, temperature_reading, voltage_reading};
    use crate::thermal;
    use crate::ui::ON_LEVEL;

    const UI_MD: &str = include_str!("../docs/ui.md");

    fn dim() -> Output {
        ramp::output(ON_LEVEL)
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

    /// On, and held at the top of the ramp.
    fn at_the_top(torch: &mut Rig) {
        click(torch);
        run(torch, true, 3 * TICKS_PER_SECOND);
        run(torch, false, RELEASE_TICKS + 1);
        assert_eq!(torch.level(), CEILING);
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

    #[test]
    fn lockout_never_lights() {
        let mut torch = Rig::default();
        clicks(&mut torch, 3);
        run(&mut torch, false, RELEASE_TICKS + 1);
        run(&mut torch, true, 2 * TICKS_PER_SECOND);
        assert_eq!(torch.level(), 0);
        run(&mut torch, false, RELEASE_TICKS + 1);
        click(&mut torch);
        assert_eq!(torch.level(), 0);
        assert_eq!(torch.aux(), Aux::Red);
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
        clicks(&mut torch, 3);
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
    fn a_flat_cell_turns_off_shows_yellow_and_ignores_clicks_until_charged() {
        let mut torch = Rig::default();
        click(&mut torch);
        torch.adc.voltage = voltage_reading(280);
        run(&mut torch, false, 10 * TICKS_PER_SECOND);
        assert_eq!(torch.level(), 0);
        assert_eq!(torch.ui.output_level(), 0, "the UI's off too");
        // Not left with the UI still on and the light out.
        assert!(torch.resting());
        // Yellow for a while after it went out, then dark.
        run(&mut torch, false, SHOW_TICKS);
        assert_eq!(torch.aux(), Aux::Off);
        // A click shows yellow again, and lights nothing.
        click(&mut torch);
        assert_eq!(torch.level(), 0);
        assert_eq!(torch.aux(), Aux::Yellow);
        run(&mut torch, false, SHOW_TICKS);
        assert_eq!(torch.aux(), Aux::Off);
        // Nor does a hold, or 3C.
        run(&mut torch, true, TICKS_PER_SECOND);
        run(&mut torch, false, RELEASE_TICKS + 1);
        clicks(&mut torch, 3);
        run(&mut torch, false, RELEASE_TICKS + 1);
        assert_eq!(torch.level(), 0);
        assert_eq!(torch.aux(), Aux::Yellow, "not locked out");
        // Charged, and the next click turns it on as if new.
        torch.adc.voltage = voltage_reading(380);
        click(&mut torch);
        assert_eq!(torch.level(), ON_LEVEL);
        click(&mut torch);
        assert_eq!(torch.aux(), Aux::Blue);
    }

    #[test]
    fn going_flat_shows_yellow_for_the_three_seconds_after() {
        let mut torch = Rig::default();
        click(&mut torch);
        torch.adc.voltage = voltage_reading(280);
        while torch.level() != 0 {
            torch.tick(false);
        }
        for _ in 0..SHOW_TICKS {
            assert_eq!(torch.aux(), Aux::Yellow);
            torch.tick(false);
        }
        assert_eq!(torch.aux(), Aux::Off);
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
    fn a_cell_it_cant_read_fades_it_out_and_shows_white() {
        let mut torch = Rig::default();
        at_the_top(&mut torch);
        torch.adc.voltage = 0;
        run(&mut torch, false, TICKS_PER_SECOND);
        // The fault's in by now, and the light's on its way down.
        assert!(torch.level() > 0 && torch.level() < CEILING);
        assert_eq!(torch.aux(), Aux::Off, "out while the main LED's lit");
        run(&mut torch, false, 2 * TICKS_PER_SECOND);
        assert_eq!(torch.level(), 0);
        assert_eq!(torch.aux(), Aux::White);
        run(&mut torch, false, SHOW_TICKS);
        assert_eq!(torch.aux(), Aux::Off);
        // A good reading changes nothing. Only a reset does.
        torch.adc.voltage = voltage_reading(380);
        click(&mut torch);
        assert_eq!(torch.level(), 0);
        assert_eq!(torch.aux(), Aux::White);
    }

    #[test]
    fn a_click_still_turns_off_a_fault_thats_fading() {
        let mut torch = Rig::default();
        at_the_top(&mut torch);
        torch.adc.voltage = 0;
        while torch.torch.limits.health() == Health::Good {
            torch.tick(false);
        }
        run(&mut torch, true, 4);
        run(&mut torch, false, RELEASE_TICKS);
        assert!(torch.level() > 0, "still fading when the click counts");
        assert_eq!(torch.tick(false), Some(Event::Click(1)));
        assert_eq!(torch.level(), 0);
    }

    #[test]
    fn heat_lowers_the_light_and_leaves_the_ui_where_it_was() {
        let mut torch = Rig::default();
        at_the_top(&mut torch);
        torch.adc.temperature = temperature_reading(thermal::celsius(80));
        run(&mut torch, false, 120 * TICKS_PER_SECOND);
        assert_eq!(torch.level(), thermal::FLOOR);
        assert_eq!(torch.output(), ramp::output(torch.level()));
        assert_eq!(torch.ui.output_level(), CEILING);
    }

    /// A switch that's stuck down from the moment the battery goes in, say a
    /// jammed button. It's a hold from off, which does nothing.
    #[test]
    fn a_switch_held_from_power_on_stays_off() {
        let mut torch = Rig::default();
        for _ in 0..10 * 60 * u32::from(TICKS_PER_SECOND) {
            torch.tick(true);
            assert_eq!(torch.level(), 0);
        }
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
    fn a_fault_never_lights_and_shows_white_after_power_on_and_a_press() {
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
            assert_eq!(torch.aux(), Aux::White, "{faults:?}");
            run(&mut torch, false, SHOW_TICKS);
            assert_eq!(torch.aux(), Aux::Off, "{faults:?}");
            click(&mut torch);
            assert_eq!(torch.level(), 0, "{faults:?}");
            assert_eq!(torch.aux(), Aux::White, "{faults:?}");
            run(&mut torch, false, SHOW_TICKS);
            // A hold shows it from its start, and no longer, however long
            // the switch stays down.
            run(&mut torch, true, 30 * TICKS_PER_SECOND);
            assert_eq!(torch.level(), 0, "{faults:?}");
            assert_eq!(torch.aux(), Aux::Off, "{faults:?}");
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
