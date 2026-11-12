//! What each click and hold does. docs/ui.md is the spec; keep them in step.

use crate::button::Event;
use crate::ramp::CEILING;

/// The lowest level the UI goes to.
///
/// The stock firmware's smooth floor on this light is 5 of 150, which is 36
/// in delta-sigma units on its table, and reviewers saw flicker below that.
/// This curve is flatter at the bottom, so its level 5 is only 24, and level
/// 13 is the first at 36 or over. Unmeasured.
pub const FLOOR: u8 = 13;

/// Where a fresh torch starts when you click it on, a duty of about 10 of 255.
pub const DEFAULT_LEVEL: u8 = 52;

// The top of the UI's ramp, and turbo, is [`CEILING`].
const _: () = assert!(FLOOR >= 1 && FLOOR < DEFAULT_LEVEL && DEFAULT_LEVEL < CEILING);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Off,
    On,
    Lockout,
}

/// What the aux LEDs on PC1 to PC3 should be doing. Any colour is dim and
/// steady, so they never report anything by flashing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Aux {
    Off,
    Blue,
    Red,
}

#[derive(Clone, Debug)]
pub struct Ui {
    mode: Mode,
    /// The level it's at while on, and the one to come back on at otherwise,
    /// since [`Ui::off`] is the only way from on to off and leaves it be.
    /// RAM only, since the torch sleeps rather than losing power, so this
    /// outlasts sleep and never needs EEPROM. A reset or a cell swap loses it.
    level: u8,
    /// The level to drop back to when leaving turbo.
    before_turbo: u8,
    /// Set when a hold switched the light on, so that the same hold doesn't
    /// go on to ramp. Cleared on release.
    swallow_hold: bool,
    /// Holding the switch in lockout for a momentary light.
    momentary: bool,
}

impl Default for Ui {
    fn default() -> Self {
        Self::new()
    }
}

impl Ui {
    pub const fn new() -> Self {
        Self {
            mode: Mode::Off,
            level: DEFAULT_LEVEL,
            before_turbo: DEFAULT_LEVEL,
            swallow_hold: false,
            momentary: false,
        }
    }

    pub const fn mode(&self) -> Mode {
        self.mode
    }

    /// The level the LEDs should be at right now, `0` meaning off.
    pub const fn output_level(&self) -> u8 {
        match self.mode {
            Mode::On => self.level,
            Mode::Lockout if self.momentary => FLOOR,
            Mode::Off | Mode::Lockout => 0,
        }
    }

    /// The aux LEDs give way to the main LED whenever it's lit.
    pub const fn aux(&self) -> Aux {
        match self.mode {
            _ if self.output_level() > 0 => Aux::Off,
            Mode::Lockout => Aux::Red,
            Mode::Off | Mode::On => Aux::Blue,
        }
    }

    pub fn handle(&mut self, event: Event) {
        match (self.mode, event) {
            (_, Event::Release { .. }) => {
                self.swallow_hold = false;
                self.momentary = false;
            }

            (Mode::Off, Event::Click(1)) => self.turn_on(self.level),
            (Mode::Off, Event::Click(2)) => {
                // With turbo remembered, this would leave 2C nowhere to go.
                if self.level < CEILING {
                    self.before_turbo = self.level;
                }
                self.turn_on(CEILING);
            }
            (Mode::Off, Event::Click(4)) => self.mode = Mode::Lockout,
            // Only the hold's first tick, so a hold that's still going when
            // `off` puts the light out can't light it again.
            (
                Mode::Off,
                Event::Hold {
                    clicks: 0,
                    ticks: 0,
                },
            ) => {
                self.turn_on(FLOOR);
                self.swallow_hold = true;
            }

            (Mode::On, Event::Click(1)) => self.off(),
            (Mode::On, Event::Click(2)) if self.level == CEILING => self.level = self.before_turbo,
            (Mode::On, Event::Click(2)) => {
                self.before_turbo = self.level;
                self.level = CEILING;
            }
            (Mode::On, Event::Hold { .. }) if self.swallow_hold => {}
            (Mode::On, Event::Hold { clicks: 0, .. }) => {
                self.level = self.level.saturating_add(1).min(CEILING);
            }
            (Mode::On, Event::Hold { clicks: 1, .. }) => {
                self.level = self.level.saturating_sub(1).max(FLOOR);
            }

            (Mode::Lockout, Event::Click(4)) => self.mode = Mode::Off,
            // The first tick only, for the same reason as from off.
            (
                Mode::Lockout,
                Event::Hold {
                    clicks: 0,
                    ticks: 0,
                },
            ) => self.momentary = true,

            _ => {}
        }
    }

    /// Puts the light out, whatever it was doing, remembering the level. This
    /// is 1C, and it's also how low-voltage protection will turn the light
    /// off. A locked torch stays locked.
    pub const fn off(&mut self) {
        match self.mode {
            Mode::On => self.mode = Mode::Off,
            Mode::Lockout => self.momentary = false,
            Mode::Off => {}
        }
    }

    const fn turn_on(&mut self, level: u8) {
        self.mode = Mode::On;
        self.level = level;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ramp::output;

    fn hold(clicks: u8, ticks: u16) -> Event {
        Event::Hold { clicks, ticks }
    }

    fn hold_for(ui: &mut Ui, clicks: u8, ticks: u16) {
        for t in 0..ticks {
            ui.handle(hold(clicks, t));
        }
        ui.handle(Event::Release { clicks });
    }

    #[test]
    fn starts_off() {
        let ui = Ui::new();
        assert_eq!(ui.mode(), Mode::Off);
        assert_eq!(ui.output_level(), 0);
    }

    /// The stock firmware's floor is 36 in these units, and the level under
    /// ours isn't.
    #[test]
    fn the_floor_is_the_first_level_at_the_stock_smooth_floor() {
        assert!(output(FLOOR).dsm >= 36);
        assert!(output(FLOOR - 1).dsm < 36);
    }

    #[test]
    fn a_fresh_torch_comes_on_at_a_duty_of_ten() {
        assert_eq!(output(DEFAULT_LEVEL).dsm >> crate::ramp::DSM_BITS, 10);
    }

    #[test]
    fn click_on_and_off_remembers_the_level() {
        let mut ui = Ui::new();
        ui.handle(Event::Click(1));
        assert_eq!(ui.output_level(), DEFAULT_LEVEL);
        hold_for(&mut ui, 0, 10);
        ui.handle(Event::Click(1));
        assert_eq!(ui.output_level(), 0);
        ui.handle(Event::Click(1));
        assert_eq!(ui.output_level(), DEFAULT_LEVEL + 10);
    }

    #[test]
    fn hold_from_off_is_the_floor_and_does_not_ramp() {
        let mut ui = Ui::new();
        hold_for(&mut ui, 0, 30);
        assert_eq!(ui.mode(), Mode::On);
        assert_eq!(ui.output_level(), FLOOR);
        hold_for(&mut ui, 0, 3);
        assert_eq!(ui.output_level(), FLOOR + 3);
    }

    #[test]
    fn ramps_stop_at_the_ends() {
        let mut ui = Ui::new();
        ui.handle(Event::Click(1));
        hold_for(&mut ui, 0, 500);
        assert_eq!(ui.output_level(), CEILING);
        hold_for(&mut ui, 1, 500);
        assert_eq!(ui.output_level(), FLOOR);
    }

    #[test]
    fn double_click_toggles_turbo() {
        let mut ui = Ui::new();
        ui.handle(Event::Click(1));
        ui.handle(Event::Click(2));
        assert_eq!(ui.output_level(), CEILING);
        ui.handle(Event::Click(2));
        assert_eq!(ui.output_level(), DEFAULT_LEVEL);
    }

    #[test]
    fn double_click_from_off_is_turbo() {
        let mut ui = Ui::new();
        ui.handle(Event::Click(2));
        assert_eq!(ui.output_level(), CEILING);
        ui.handle(Event::Click(2));
        assert_eq!(ui.output_level(), DEFAULT_LEVEL);
    }

    #[test]
    fn turbo_is_remembered_and_2c_still_has_somewhere_to_go() {
        let mut ui = Ui::new();
        ui.handle(Event::Click(2));
        ui.handle(Event::Click(1));
        ui.handle(Event::Click(2));
        assert_eq!(ui.output_level(), CEILING);
        ui.handle(Event::Click(2));
        assert_eq!(ui.output_level(), DEFAULT_LEVEL);
    }

    #[test]
    fn lockout_ignores_clicks_and_allows_a_momentary_light() {
        let mut ui = Ui::new();
        ui.handle(Event::Click(4));
        assert_eq!(ui.mode(), Mode::Lockout);
        ui.handle(Event::Click(1));
        ui.handle(Event::Click(2));
        assert_eq!(ui.output_level(), 0);
        ui.handle(hold(0, 0));
        assert_eq!(ui.output_level(), FLOOR);
        ui.handle(Event::Release { clicks: 0 });
        assert_eq!(ui.output_level(), 0);
        ui.handle(Event::Click(4));
        assert_eq!(ui.mode(), Mode::Off);
    }

    #[test]
    fn off_remembers_the_level_and_1c_still_turns_it_on() {
        let mut ui = Ui::new();
        ui.handle(Event::Click(1));
        hold_for(&mut ui, 0, 10);
        ui.off();
        assert_eq!(ui.output_level(), 0);
        ui.handle(Event::Click(1));
        assert_eq!(ui.output_level(), DEFAULT_LEVEL + 10);
    }

    #[test]
    fn a_hold_that_outlasts_off_does_not_light_it_again() {
        let mut ui = Ui::new();
        ui.handle(Event::Click(1));
        ui.handle(hold(0, 0));
        ui.off();
        ui.handle(hold(0, 1));
        assert_eq!(ui.output_level(), 0);
        ui.handle(Event::Release { clicks: 0 });
        ui.handle(hold(0, 0));
        assert_eq!(ui.output_level(), FLOOR);
    }

    #[test]
    fn off_leaves_a_locked_torch_locked() {
        let mut ui = Ui::new();
        ui.handle(Event::Click(4));
        ui.handle(hold(0, 0));
        ui.off();
        ui.handle(hold(0, 1));
        assert_eq!(ui.mode(), Mode::Lockout);
        assert_eq!(ui.output_level(), 0);
    }

    #[test]
    fn aux_is_blue_when_off_red_in_lockout_and_out_with_the_light_on() {
        let mut ui = Ui::new();
        assert_eq!(ui.aux(), Aux::Blue);
        ui.handle(Event::Click(1));
        assert_eq!(ui.aux(), Aux::Off);
        ui.handle(Event::Click(1));
        ui.handle(Event::Click(4));
        assert_eq!(ui.aux(), Aux::Red);
        ui.handle(hold(0, 0));
        assert_eq!(ui.aux(), Aux::Off);
        ui.handle(Event::Release { clicks: 0 });
        assert_eq!(ui.aux(), Aux::Red);
    }
}
