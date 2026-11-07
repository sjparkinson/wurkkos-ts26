//! One pass of the firmware's loop, minus the hardware: a switch sample and
//! an ADC result in, a level out.

use crate::button::{Clicks, Event};
use crate::ramp::{self, Output};
use crate::ui::{Aux, Ui};

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

    /// One 64 Hz tick, `pressed` meaning the switch is down and `adc` the
    /// ADC's result, if it has one ready. Returns the click or hold this
    /// sample finished, if any.
    ///
    /// Nothing reads `adc` until `limits::Limits` lands. That's the seam: it
    /// takes `self.ui.output_level()` and `adc` here, [`Torch::level`] returns
    /// its answer, its `off` calls `self.ui.off()`, and [`Torch::resting`]
    /// checks its level too.
    pub fn tick(&mut self, pressed: bool, _adc: Option<u16>) -> Option<Event> {
        let event = self.clicks.tick(pressed);
        if let Some(event) = event {
            self.ui.handle(event);
        }
        event
    }

    /// Whether there's nothing to do until the switch moves: the light's off
    /// and no click is being counted.
    pub fn resting(&self) -> bool {
        self.clicks.idle() && self.level() == 0
    }

    /// The level the LEDs should be at right now, `0` meaning off.
    pub fn level(&self) -> u8 {
        self.ui.output_level()
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
    use crate::ui::{CEIL, DEFAULT_LEVEL, FLOOR};

    const UI_MD: &str = include_str!("../docs/ui.md");

    fn dim() -> Output {
        ramp::output(DEFAULT_LEVEL)
    }

    fn run(torch: &mut Torch, pressed: bool, ticks: u16) {
        for _ in 0..ticks {
            torch.tick(pressed, None);
        }
    }

    fn clicks(torch: &mut Torch, n: u8) {
        for _ in 0..n {
            run(torch, true, 4);
            run(torch, false, 4);
        }
    }

    fn click(torch: &mut Torch) {
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
        let mut torch = Torch::new();
        for word in script.split_whitespace() {
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
        assert!(checked >= 16, "only found {checked} examples");
    }

    #[test]
    fn starts_off() {
        let mut torch = Torch::new();
        assert_eq!(torch.tick(false, None), None);
        assert_eq!(torch.level(), 0);
        assert_eq!(torch.output(), Output::OFF);
    }

    #[test]
    fn a_click_turns_it_on_once_the_click_counts() {
        let mut torch = Torch::new();
        run(&mut torch, true, 4);
        run(&mut torch, false, RELEASE_TICKS);
        assert_eq!(torch.output(), Output::OFF);
        assert_eq!(torch.tick(false, None), Some(Event::Click(1)));
        assert_eq!(torch.output(), dim());
    }

    #[test]
    fn a_second_click_turns_it_off() {
        let mut torch = Torch::new();
        click(&mut torch);
        click(&mut torch);
        assert_eq!(torch.output(), Output::OFF);
    }

    /// Lockout: 1H. A script can only see the light after the hold ends.
    #[test]
    fn lockout_lights_at_the_floor_only_while_held() {
        let mut torch = Torch::new();
        clicks(&mut torch, 4);
        run(&mut torch, false, RELEASE_TICKS + 1);
        run(&mut torch, true, 2 * TICKS_PER_SECOND);
        assert_eq!(torch.level(), FLOOR);
        run(&mut torch, false, 1);
        assert_eq!(torch.level(), 0);
    }

    #[test]
    fn the_ramp_never_asks_for_more_than_the_ceiling() {
        let mut torch = Torch::new();
        click(&mut torch);
        run(&mut torch, true, 10 * TICKS_PER_SECOND);
        assert_eq!(torch.level(), CEIL);
        assert_eq!(torch.output(), ramp::output(ramp::CEILING));
    }

    #[test]
    fn rests_only_with_the_light_off_and_no_click_counting() {
        let mut torch = Torch::new();
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
        let mut torch = Torch::new();
        clicks(&mut torch, 4);
        run(&mut torch, false, RELEASE_TICKS + 1);
        assert!(torch.resting());
    }

    #[test]
    fn the_aux_leds_follow_the_ui() {
        let mut torch = Torch::new();
        assert_eq!(torch.aux(), Aux::Blue);
        click(&mut torch);
        assert_eq!(torch.aux(), Aux::Off);
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
