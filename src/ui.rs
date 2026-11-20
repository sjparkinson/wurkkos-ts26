use crate::button::Event;
use crate::ramp::CEILING;

/// 24 delta-sigma units, under the stock firmware's smooth floor of 36,
/// below which reviewers saw flicker. Unmeasured for flicker on this torch.
pub const FLOOR: u8 = 5;

pub const ON_LEVEL: u8 = 52;

const _: () = assert!(FLOOR >= 1 && FLOOR < ON_LEVEL && ON_LEVEL < CEILING);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Off,
    On,
    Lockout,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Aux {
    Off,
    Blue,
    /// Bright, since dim red is hard to see, so it's only shown for
    /// [`crate::torch::LOCKED_TICKS`].
    Red,
    Yellow,
    White,
}

#[derive(Clone, Debug)]
pub struct Ui {
    mode: Mode,
    level: u8,
    /// Whether the last 1H ramped up.
    up: bool,
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
            level: ON_LEVEL,
            up: false,
        }
    }

    pub const fn mode(&self) -> Mode {
        self.mode
    }

    pub const fn output_level(&self) -> u8 {
        match self.mode {
            Mode::On => self.level,
            Mode::Off | Mode::Lockout => 0,
        }
    }

    /// Locked, it waits the full time, so a fourth press in a pocket is a 4C
    /// rather than a 3C that unlocks it.
    pub const fn most_clicks(&self) -> Option<u8> {
        match self.mode {
            Mode::On => Some(1),
            Mode::Off => Some(3),
            Mode::Lockout => None,
        }
    }

    pub const fn aux(&self) -> Aux {
        match self.mode {
            Mode::On => Aux::Off,
            Mode::Lockout => Aux::Red,
            Mode::Off => Aux::Blue,
        }
    }

    pub fn handle(&mut self, event: Event) {
        match (self.mode, event) {
            (Mode::Off, Event::Click(1)) => {
                self.mode = Mode::On;
                self.level = ON_LEVEL;
                self.up = false;
            }
            (Mode::Off, Event::Click(3)) => self.mode = Mode::Lockout,
            (
                Mode::Off,
                Event::Hold {
                    clicks: 0,
                    ticks: 0,
                },
            ) => {
                self.mode = Mode::On;
                self.level = FLOOR;
                self.up = false;
            }

            (Mode::On, Event::Click(1)) => self.off(),
            (Mode::On, Event::Hold { clicks: 0, ticks }) => {
                if ticks == 0 {
                    self.up = !self.up;
                }
                self.level = if self.up {
                    self.level.saturating_add(1).min(CEILING)
                } else {
                    self.level.saturating_sub(1).max(FLOOR)
                };
            }

            (Mode::Lockout, Event::Click(3)) => self.mode = Mode::Off,

            _ => {}
        }
    }

    pub const fn off(&mut self) {
        if matches!(self.mode, Mode::On) {
            self.mode = Mode::Off;
        }
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

    #[test]
    fn it_comes_on_at_a_duty_of_ten() {
        assert_eq!(output(ON_LEVEL).dsm >> crate::ramp::DSM_BITS, 10);
    }

    #[test]
    fn click_on_and_off_forgets_the_level() {
        let mut ui = Ui::new();
        ui.handle(Event::Click(1));
        assert_eq!(ui.output_level(), ON_LEVEL);
        hold_for(&mut ui, 0, 10);
        assert_eq!(ui.output_level(), ON_LEVEL + 10);
        ui.handle(Event::Click(1));
        assert_eq!(ui.output_level(), 0);
        ui.handle(Event::Click(1));
        assert_eq!(ui.output_level(), ON_LEVEL);
    }

    #[test]
    fn off_answers_to_1c_3c_and_1h_and_nothing_else() {
        for event in [
            Event::Click(2),
            Event::Click(4),
            Event::Click(5),
            hold(0, 1),
            hold(1, 0),
            hold(2, 0),
        ] {
            let mut ui = Ui::new();
            ui.handle(event);
            assert_eq!(ui.mode(), Mode::Off, "{event:?}");
        }
    }

    #[test]
    fn a_1h_from_off_lights_the_floor_and_stays_there() {
        let mut ui = Ui::new();
        for ticks in 0..500 {
            ui.handle(hold(0, ticks));
            assert_eq!(ui.output_level(), FLOOR, "{ticks}");
        }
    }

    #[test]
    fn the_1h_after_a_1h_from_off_goes_up() {
        let mut ui = Ui::new();
        hold_for(&mut ui, 0, 5);
        hold_for(&mut ui, 0, 10);
        assert_eq!(ui.output_level(), FLOOR + 10);
        hold_for(&mut ui, 0, 4);
        assert_eq!(ui.output_level(), FLOOR + 6);
    }

    #[test]
    fn ramps_stop_at_the_ends() {
        let mut ui = Ui::new();
        ui.handle(Event::Click(1));
        hold_for(&mut ui, 0, 500);
        assert_eq!(ui.output_level(), CEILING);
        hold_for(&mut ui, 0, 500);
        assert_eq!(ui.output_level(), FLOOR);
    }

    #[test]
    fn each_1h_ramps_the_other_way_from_the_last() {
        let mut ui = Ui::new();
        ui.handle(Event::Click(1));
        hold_for(&mut ui, 0, 10);
        assert_eq!(ui.output_level(), ON_LEVEL + 10);
        hold_for(&mut ui, 0, 4);
        assert_eq!(ui.output_level(), ON_LEVEL + 6);
        hold_for(&mut ui, 0, 2);
        assert_eq!(ui.output_level(), ON_LEVEL + 8);
    }

    #[test]
    fn the_first_1h_after_a_click_on_goes_up() {
        let mut ui = Ui::new();
        ui.handle(Event::Click(1));
        hold_for(&mut ui, 0, 10);
        ui.handle(Event::Click(1));
        ui.handle(Event::Click(1));
        hold_for(&mut ui, 0, 3);
        assert_eq!(ui.output_level(), ON_LEVEL + 3);
    }

    #[test]
    fn a_2h_does_nothing_while_on_and_does_not_count_as_a_1h() {
        let mut ui = Ui::new();
        ui.handle(Event::Click(1));
        hold_for(&mut ui, 1, 100);
        assert_eq!(ui.output_level(), ON_LEVEL);
        hold_for(&mut ui, 0, 3);
        assert_eq!(ui.output_level(), ON_LEVEL + 3);
    }

    #[test]
    fn on_ignores_other_clicks() {
        for clicks in [2, 3, 4, 5] {
            let mut ui = Ui::new();
            ui.handle(Event::Click(1));
            ui.handle(Event::Click(clicks));
            assert_eq!(ui.output_level(), ON_LEVEL, "{clicks}C");
        }
    }

    #[test]
    fn nothing_past_most_clicks_does_anything() {
        for setup in [&[][..], &[Event::Click(1)], &[Event::Click(3)]] {
            let mut ui = Ui::new();
            for &event in setup {
                ui.handle(event);
            }
            let Some(most) = ui.most_clicks() else {
                continue;
            };
            for n in most + 1..=most + 4 {
                let mut after = ui.clone();
                after.handle(Event::Click(n));
                after.handle(hold(n, 0));
                assert_eq!(after.mode(), ui.mode(), "{n}C");
                assert_eq!(after.output_level(), ui.output_level(), "{n}C");
            }
        }
    }

    #[test]
    fn lockout_never_lights_and_3c_unlocks() {
        let mut ui = Ui::new();
        ui.handle(Event::Click(3));
        assert_eq!(ui.mode(), Mode::Lockout);
        for event in [Event::Click(1), Event::Click(2), Event::Click(4)] {
            ui.handle(event);
        }
        hold_for(&mut ui, 0, 100);
        hold_for(&mut ui, 1, 100);
        assert_eq!(ui.mode(), Mode::Lockout);
        assert_eq!(ui.output_level(), 0);
        ui.handle(Event::Click(3));
        assert_eq!(ui.mode(), Mode::Off);
    }

    #[test]
    fn a_hold_that_outlasts_off_does_not_light_it_again() {
        let mut ui = Ui::new();
        ui.handle(Event::Click(1));
        ui.handle(hold(0, 0));
        ui.off();
        ui.handle(hold(0, 1));
        assert_eq!(ui.output_level(), 0);
    }

    #[test]
    fn off_leaves_a_locked_torch_locked() {
        let mut ui = Ui::new();
        ui.handle(Event::Click(3));
        ui.off();
        assert_eq!(ui.mode(), Mode::Lockout);
    }

    #[test]
    fn aux_is_blue_when_off_red_in_lockout_and_out_with_the_light_on() {
        let mut ui = Ui::new();
        assert_eq!(ui.aux(), Aux::Blue);
        ui.handle(Event::Click(1));
        assert_eq!(ui.aux(), Aux::Off);
        ui.handle(Event::Click(1));
        ui.handle(Event::Click(3));
        assert_eq!(ui.aux(), Aux::Red);
    }
}
