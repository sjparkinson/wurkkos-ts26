use crate::button::{Count, Event};
use crate::ramp::CEILING;

/// Levels 5, 11 and 13 all flickered slightly on this torch.
pub const FLOOR: u8 = 17;

pub const ON_LEVEL: u8 = 52;

const _: () = assert!(FLOOR >= 1 && FLOOR < ON_LEVEL && ON_LEVEL < CEILING);

pub const TWO_H: Event = Event::Hold {
    clicks: 1,
    ticks: 0,
};

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
    Red,
    Yellow,
    White,
}

#[derive(Clone, Debug)]
pub struct Ui {
    mode: Mode,
    level: u8,
    up: bool,
}

impl Default for Ui {
    fn default() -> Self {
        Self::new()
    }
}

impl Ui {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            mode: Mode::Off,
            level: ON_LEVEL,
            up: false,
        }
    }

    #[must_use]
    pub const fn mode(&self) -> Mode {
        self.mode
    }

    #[must_use]
    pub const fn output_level(&self) -> u8 {
        match self.mode {
            Mode::On => self.level,
            Mode::Off | Mode::Lockout => 0,
        }
    }

    #[must_use]
    pub const fn count(&self) -> Count {
        match self.mode {
            Mode::On | Mode::Off => Count::Each,
            Mode::Lockout => Count::Group,
        }
    }

    #[must_use]
    pub const fn aux(&self) -> Aux {
        match self.mode {
            Mode::On => Aux::Off,
            Mode::Lockout => Aux::Red,
            Mode::Off => Aux::Blue,
        }
    }

    #[must_use]
    pub const fn lights(&self, event: Event) -> u8 {
        match (self.mode, event) {
            (Mode::Off, Event::Click(1)) => ON_LEVEL,
            (
                Mode::Off,
                Event::Hold {
                    clicks: 0,
                    ticks: 0,
                },
            ) => FLOOR,
            _ => 0,
        }
    }

    pub fn handle_unlit(&mut self, event: Event) -> bool {
        let locks = matches!(self.mode, Mode::Off | Mode::On) && event == TWO_H;
        if locks {
            self.mode = Mode::Lockout;
        }
        locks
    }

    pub fn handle(&mut self, event: Event) {
        if self.handle_unlit(event) {
            return;
        }
        match (self.mode, event) {
            (Mode::Off, Event::Click(1)) => {
                self.mode = Mode::On;
                self.level = ON_LEVEL;
                self.up = false;
            }
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

            (Mode::Lockout, TWO_H) => self.mode = Mode::Off,

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

    const LOCK: [Event; 3] = [Event::Click(1), Event::Click(1), TWO_H];

    fn lock(ui: &mut Ui) {
        for event in LOCK {
            ui.handle(event);
        }
        assert_eq!(ui.mode(), Mode::Lockout);
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
    fn clicks_count_one_at_a_time_unless_locked() {
        let mut ui = Ui::new();
        assert_eq!(ui.count(), Count::Each);
        ui.handle(Event::Click(1));
        assert_eq!(ui.count(), Count::Each);
        lock(&mut ui);
        assert_eq!(ui.count(), Count::Group);
    }

    #[test]
    fn off_answers_to_1c_1h_and_2h_and_nothing_else() {
        for event in [
            Event::Click(2),
            Event::Click(3),
            hold(0, 1),
            hold(1, 1),
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
    fn on_ignores_other_clicks_and_holds_after_more_than_one() {
        for event in [Event::Click(2), Event::Click(3), hold(2, 0), hold(3, 0)] {
            let mut ui = Ui::new();
            ui.handle(Event::Click(1));
            ui.handle(event);
            assert_eq!(ui.output_level(), ON_LEVEL, "{event:?}");
        }
    }

    #[test]
    fn a_2h_with_the_light_on_puts_it_out_and_locks_it() {
        let mut ui = Ui::new();
        ui.handle(Event::Click(1));
        ui.handle(TWO_H);
        assert_eq!(ui.mode(), Mode::Lockout);
        assert_eq!(ui.output_level(), 0);
    }

    #[test]
    fn a_2h_from_off_locks_it() {
        let mut ui = Ui::new();
        ui.handle(TWO_H);
        assert_eq!(ui.mode(), Mode::Lockout);
    }

    #[test]
    fn the_hold_that_locks_it_does_not_unlock_it() {
        let mut ui = Ui::new();
        hold_for(&mut ui, 1, 500);
        assert_eq!(ui.mode(), Mode::Lockout);
    }

    #[test]
    fn a_light_that_was_put_out_still_locks_on_a_2h() {
        let mut ui = Ui::new();
        ui.handle(Event::Click(1));
        ui.off();
        ui.handle(TWO_H);
        assert_eq!(ui.mode(), Mode::Lockout);
    }

    #[test]
    fn handled_unlit_it_only_ever_locks_and_never_lights() {
        for event in [
            Event::Click(1),
            Event::Click(3),
            hold(0, 0),
            hold(0, 1),
            TWO_H,
        ] {
            for setup in [&[][..], &[Event::Click(1)], &LOCK] {
                let mut ui = Ui::new();
                for &event in setup {
                    ui.handle(event);
                }
                let (before, level) = (ui.mode(), ui.output_level());
                let locks = ui.handle_unlit(event);
                assert_eq!(locks, event == TWO_H && before != Mode::Lockout);
                let after = if locks {
                    (Mode::Lockout, 0)
                } else {
                    (before, level)
                };
                assert_eq!(
                    (ui.mode(), ui.output_level()),
                    after,
                    "{event:?} from {before:?}"
                );
            }
        }
    }

    #[test]
    fn lockout_never_lights_and_a_2h_unlocks_it_to_off() {
        let mut ui = Ui::new();
        lock(&mut ui);
        for event in [
            Event::Click(1),
            Event::Click(2),
            Event::Click(3),
            Event::Click(4),
        ] {
            ui.handle(event);
        }
        hold_for(&mut ui, 0, 100);
        hold_for(&mut ui, 2, 100);
        assert_eq!(ui.mode(), Mode::Lockout);
        assert_eq!(ui.output_level(), 0);
        hold_for(&mut ui, 1, 100);
        assert_eq!(ui.mode(), Mode::Off);
        assert_eq!(ui.output_level(), 0);
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
        lock(&mut ui);
        ui.off();
        assert_eq!(ui.mode(), Mode::Lockout);
    }

    #[test]
    fn aux_is_blue_when_off_red_in_lockout_and_out_with_the_light_on() {
        let mut ui = Ui::new();
        assert_eq!(ui.aux(), Aux::Blue);
        ui.handle(Event::Click(1));
        assert_eq!(ui.aux(), Aux::Off);
        ui.handle(TWO_H);
        assert_eq!(ui.aux(), Aux::Red);
    }
}
