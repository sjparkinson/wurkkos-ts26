//! What each click and hold does. docs/ui.md is the spec; keep them in step.
//!
//! For now it's the smallest UI that proves the hardware works: one click on,
//! one click off, and nothing else. What "on" drives is up to `src/torch.rs`.

use crate::button::Event;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Off,
    On,
}

#[derive(Clone, Debug)]
pub struct Ui {
    mode: Mode,
}

impl Default for Ui {
    fn default() -> Self {
        Self::new()
    }
}

impl Ui {
    pub const fn new() -> Self {
        Self { mode: Mode::Off }
    }

    pub fn mode(&self) -> Mode {
        self.mode
    }

    pub fn handle(&mut self, event: Event) {
        self.mode = match (self.mode, event) {
            (Mode::Off, Event::Click(1)) => Mode::On,
            (Mode::On, Event::Click(1)) => Mode::Off,
            (mode, _) => mode,
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_off() {
        assert_eq!(Ui::new().mode(), Mode::Off);
    }

    /// Off: 1C. On: 1C.
    #[test]
    fn one_click_turns_on_and_off() {
        let mut ui = Ui::new();
        ui.handle(Event::Click(1));
        assert_eq!(ui.mode(), Mode::On);
        ui.handle(Event::Click(1));
        assert_eq!(ui.mode(), Mode::Off);
    }

    /// Off and On: anything else.
    #[test]
    fn everything_else_does_nothing() {
        let others = [
            Event::Click(2),
            Event::Click(3),
            Event::Click(4),
            Event::Click(u8::MAX),
            Event::Hold {
                clicks: 0,
                ticks: 0,
            },
            Event::Hold {
                clicks: 1,
                ticks: 40,
            },
            Event::Release { clicks: 0 },
            Event::Release { clicks: 1 },
        ];
        for start in [Mode::Off, Mode::On] {
            for event in others {
                let mut ui = Ui { mode: start };
                ui.handle(event);
                assert_eq!(ui.mode(), start, "{event:?} from {start:?}");
            }
        }
    }
}
