//! One pass of the firmware's loop, minus the hardware: a switch sample in,
//! what the boost converter and its PWM should be doing out.

use crate::button::{Clicks, Event};
use crate::ramp::{self, Output};
use crate::ui::{Mode, Ui};

/// The one level the torch runs at, a duty of about 10 of 255. Dim on purpose,
/// until someone's measured what the MP3432 does with PWM.
pub const DIM_LEVEL: u8 = 52;

#[derive(Clone, Debug, Default)]
pub struct Torch {
    clicks: Clicks,
    ui: Ui,
}

impl Torch {
    pub const fn new() -> Self {
        Self {
            clicks: Clicks::new(),
            ui: Ui::new(),
        }
    }

    /// One 64 Hz tick, `pressed` meaning the switch is down. Returns the
    /// click or hold this sample finished, if any.
    pub fn tick(&mut self, pressed: bool) -> Option<Event> {
        let event = self.clicks.tick(pressed);
        if let Some(event) = event {
            self.ui.handle(event);
        }
        event
    }

    pub fn output(&self) -> Output {
        match self.ui.mode() {
            Mode::Off => Output::OFF,
            Mode::On => ramp::output(DIM_LEVEL),
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
    use crate::ramp::DSM_BITS;

    fn dim() -> Output {
        ramp::output(DIM_LEVEL)
    }

    fn run(torch: &mut Torch, pressed: bool, ticks: u16) {
        for _ in 0..ticks {
            torch.tick(pressed);
        }
    }

    fn click(torch: &mut Torch) {
        run(torch, true, 4);
        run(torch, false, RELEASE_TICKS + 1);
    }

    #[test]
    fn starts_off() {
        let mut torch = Torch::new();
        assert_eq!(torch.tick(false), None);
        assert_eq!(torch.output(), Output::OFF);
    }

    #[test]
    fn off_means_the_boost_converter_is_off_and_the_duty_is_zero() {
        assert_eq!(
            Output::OFF,
            Output {
                enable: false,
                dsm: 0
            }
        );
    }

    #[test]
    fn on_means_the_boost_converter_is_on_at_a_duty_of_ten() {
        assert!(dim().enable);
        assert_eq!(dim().dsm >> DSM_BITS, 10);
    }

    #[test]
    fn a_click_turns_it_on_once_the_click_counts() {
        let mut torch = Torch::new();
        run(&mut torch, true, 4);
        run(&mut torch, false, RELEASE_TICKS);
        assert_eq!(torch.output(), Output::OFF);
        assert_eq!(torch.tick(false), Some(Event::Click(1)));
        assert_eq!(torch.output(), dim());
    }

    #[test]
    fn a_second_click_turns_it_off() {
        let mut torch = Torch::new();
        click(&mut torch);
        click(&mut torch);
        assert_eq!(torch.output(), Output::OFF);
    }

    #[test]
    fn holding_does_nothing() {
        let mut torch = Torch::new();
        run(&mut torch, true, 3 * 64);
        run(&mut torch, false, RELEASE_TICKS + 1);
        assert_eq!(torch.output(), Output::OFF);
        click(&mut torch);
        run(&mut torch, true, 3 * 64);
        run(&mut torch, false, RELEASE_TICKS + 1);
        assert_eq!(torch.output(), dim());
    }

    #[test]
    fn still_turns_off_after_hours_on() {
        let mut torch = Torch::new();
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
