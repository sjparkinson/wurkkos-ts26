//! Turns one switch sample per tick into clicks and holds.
//!
//! Sampling at 64 Hz debounces the switch for free, since contact bounce is
//! over well within the 15.6 ms between samples.

use crate::TICKS_PER_SECOND;

/// Press for this long and it's a hold, not a click (about 375 ms).
/// Counted from the tick the press is first seen, which is tick 0.
pub const HOLD_TICKS: u16 = TICKS_PER_SECOND * 3 / 8;

/// Leave the switch alone this long and the clicks so far count (about 280 ms).
pub const RELEASE_TICKS: u16 = TICKS_PER_SECOND * 9 / 32;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    /// `n` quick clicks, then nothing for [`RELEASE_TICKS`].
    Click(u8),
    /// The switch is being held after `clicks` quick clicks (so `2` is
    /// click, click, hold). Sent every tick while held; `ticks` counts from
    /// the moment the press became a hold.
    Hold { clicks: u8, ticks: u16 },
    /// A hold ended.
    Release { clicks: u8 },
}

#[derive(Clone, Debug, Default)]
pub struct Clicks {
    pressed: bool,
    count: u8,
    ticks: u16,
}

impl Clicks {
    pub const fn new() -> Self {
        Self {
            pressed: false,
            count: 0,
            ticks: 0,
        }
    }

    /// Whether the switch is up with no clicks waiting to count.
    pub const fn idle(&self) -> bool {
        !self.pressed && self.count == 0
    }

    /// Feeds in one sample, `true` meaning the switch is down.
    pub fn tick(&mut self, pressed: bool) -> Option<Event> {
        self.ticks = self.ticks.saturating_add(1);

        match (self.pressed, pressed) {
            (false, true) => {
                self.pressed = true;
                self.ticks = 0;
                None
            }
            (true, true) => self.ticks.checked_sub(HOLD_TICKS).map(|ticks| Event::Hold {
                clicks: self.count,
                ticks,
            }),
            (true, false) => {
                // `ticks` has gone up once since the last pressed tick, so
                // a press that sent Holds is now past HOLD_TICKS.
                let held = self.ticks > HOLD_TICKS;
                self.pressed = false;
                self.ticks = 0;
                if held {
                    Some(Event::Release {
                        clicks: core::mem::take(&mut self.count),
                    })
                } else {
                    self.count = self.count.saturating_add(1);
                    None
                }
            }
            (false, false) if self.count > 0 && self.ticks >= RELEASE_TICKS => {
                Some(Event::Click(core::mem::take(&mut self.count)))
            }
            (false, false) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    extern crate std;

    use super::*;
    use std::vec::Vec;

    /// Runs a script of (pressed, ticks) spans and collects the events.
    fn run(script: &[(bool, u16)]) -> ([Option<Event>; 64], usize) {
        let mut clicks = Clicks::new();
        let mut events = [None; 64];
        let mut n = 0;
        for &(pressed, ticks) in script {
            for _ in 0..ticks {
                if let Some(event) = clicks.tick(pressed) {
                    if n < events.len() {
                        events[n] = Some(event);
                    }
                    n += 1;
                }
            }
        }
        (events, n)
    }

    #[test]
    fn idle_until_a_press_and_again_once_the_clicks_count() {
        let mut clicks = Clicks::new();
        assert!(clicks.idle());
        clicks.tick(true);
        assert!(!clicks.idle(), "pressed");
        clicks.tick(false);
        assert!(!clicks.idle(), "a click waiting to count");
        for _ in 0..RELEASE_TICKS - 1 {
            assert_eq!(clicks.tick(false), None);
            assert!(!clicks.idle());
        }
        assert_eq!(clicks.tick(false), Some(Event::Click(1)));
        assert!(clicks.idle());
    }

    #[test]
    fn idle_once_a_hold_ends() {
        let mut clicks = Clicks::new();
        for _ in 0..=HOLD_TICKS {
            clicks.tick(true);
            assert!(!clicks.idle());
        }
        assert_eq!(clicks.tick(false), Some(Event::Release { clicks: 0 }));
        assert!(clicks.idle());
    }

    #[test]
    fn single_click() {
        let (events, n) = run(&[(true, 5), (false, RELEASE_TICKS + 5)]);
        assert_eq!(n, 1);
        assert_eq!(events[0], Some(Event::Click(1)));
    }

    #[test]
    fn three_clicks() {
        let (events, n) = run(&[
            (true, 4),
            (false, 6),
            (true, 4),
            (false, 6),
            (true, 4),
            (false, RELEASE_TICKS + 1),
        ]);
        assert_eq!(n, 1);
        assert_eq!(events[0], Some(Event::Click(3)));
    }

    #[test]
    fn nothing_until_the_release_timeout() {
        let (_, n) = run(&[(true, 4), (false, RELEASE_TICKS - 1)]);
        assert_eq!(n, 0);
    }

    #[test]
    fn hold_then_release() {
        let (events, n) = run(&[(true, HOLD_TICKS + 4), (false, RELEASE_TICKS + 1)]);
        assert_eq!(n, 4 + 1);
        assert_eq!(
            events[0],
            Some(Event::Hold {
                clicks: 0,
                ticks: 0
            })
        );
        assert_eq!(
            events[3],
            Some(Event::Hold {
                clicks: 0,
                ticks: 3
            })
        );
        assert_eq!(events[4], Some(Event::Release { clicks: 0 }));
    }

    #[test]
    fn click_then_hold() {
        let (events, n) = run(&[(true, 4), (false, 6), (true, HOLD_TICKS + 2), (false, 1)]);
        assert_eq!(n, 3);
        assert_eq!(
            events[0],
            Some(Event::Hold {
                clicks: 1,
                ticks: 0
            })
        );
        assert_eq!(events[2], Some(Event::Release { clicks: 1 }));
    }

    #[test]
    fn a_click_after_a_hold_starts_from_zero() {
        let (events, n) = run(&[
            (true, HOLD_TICKS + 1),
            (false, 6),
            (true, 4),
            (false, RELEASE_TICKS + 1),
        ]);
        assert_eq!(events[n - 1], Some(Event::Click(1)));
    }

    #[test]
    fn a_press_becomes_a_hold_after_375_ms() {
        // docs/ui.md promises about 375 ms, which is 24 ticks at 64 Hz.
        assert_eq!(TICKS_PER_SECOND, 64);
        assert_eq!(HOLD_TICKS, 24);
        assert_eq!(
            u32::from(HOLD_TICKS) * 1000 / u32::from(TICKS_PER_SECOND),
            375
        );
    }

    #[test]
    fn clicks_count_after_the_switch_is_left_alone_for_about_280_ms() {
        // docs/ui.md promises about 280 ms, which is 18 ticks at 64 Hz.
        assert_eq!(RELEASE_TICKS, 18);
        assert_eq!(
            u32::from(RELEASE_TICKS) * 1000 / u32::from(TICKS_PER_SECOND),
            281
        );
    }

    #[test]
    fn a_press_let_go_just_before_it_becomes_a_hold_is_a_click() {
        // Down for ticks 0 to HOLD_TICKS - 1, so the tick that would have
        // been the first Hold sees the switch up.
        let (events, n) = run(&[(true, HOLD_TICKS), (false, RELEASE_TICKS + 1)]);
        assert_eq!(n, 1);
        assert_eq!(events[0], Some(Event::Click(1)));
    }

    #[test]
    fn a_press_let_go_just_after_it_becomes_a_hold_is_a_hold() {
        // One tick longer, and it sends one Hold and then a Release, and
        // never counts as a click.
        let (events, n) = run(&[(true, HOLD_TICKS + 1), (false, RELEASE_TICKS + 1)]);
        assert_eq!(n, 2);
        assert_eq!(
            events[0],
            Some(Event::Hold {
                clicks: 0,
                ticks: 0
            })
        );
        assert_eq!(events[1], Some(Event::Release { clicks: 0 }));
    }

    #[test]
    fn the_click_count_stops_at_255_rather_than_wrapping() {
        for extra in [0, 1, 45] {
            let mut clicks = Clicks::new();
            for _ in 0..255 + extra {
                assert_eq!(clicks.tick(true), None);
                assert_eq!(clicks.tick(false), None);
            }
            for _ in 0..RELEASE_TICKS - 1 {
                assert_eq!(clicks.tick(false), None);
            }
            assert_eq!(clicks.tick(false), Some(Event::Click(255)), "{extra} over");
            assert!(clicks.idle());
        }
    }

    #[test]
    fn a_hold_after_more_than_255_clicks_still_reports_255() {
        let mut clicks = Clicks::new();
        for _ in 0..300 {
            clicks.tick(true);
            clicks.tick(false);
        }
        for _ in 0..HOLD_TICKS {
            assert_eq!(clicks.tick(true), None);
        }
        assert_eq!(
            clicks.tick(true),
            Some(Event::Hold {
                clicks: 255,
                ticks: 0
            })
        );
        assert_eq!(clicks.tick(false), Some(Event::Release { clicks: 255 }));
        assert!(clicks.idle());
    }

    /// What one tick should give: the event, and whether the decoder is idle
    /// after it.
    type Expected = (Option<Event>, bool);

    /// The decoder as docs/ui.md and the doc comments describe it, worked
    /// out from whole runs of the switch being down or up rather than a
    /// sample at a time. The runs must alternate.
    ///
    /// A press of more than [`HOLD_TICKS`] samples is a hold: it sends a
    /// Hold on every sample from the one numbered `HOLD_TICKS` (the first
    /// being 0), and a Release on the first sample after it. Any shorter
    /// press is a click. Clicks add up until the switch has been up for
    /// `RELEASE_TICKS` samples after the one that saw it come up, and a hold
    /// takes the clicks before it with it.
    fn model(runs: &[(bool, u16)]) -> Vec<Expected> {
        let mut expected = Vec::new();
        // Clicks waiting to count.
        let mut waiting = 0u8;
        // Whether the run before this one was a press that became a hold.
        let mut after_hold = false;
        for (index, &(pressed, len)) in runs.iter().enumerate() {
            if pressed {
                for sample in 0..len {
                    let event = (sample >= HOLD_TICKS).then(|| Event::Hold {
                        clicks: waiting,
                        ticks: sample - HOLD_TICKS,
                    });
                    expected.push((event, false));
                }
                after_hold = len > HOLD_TICKS;
                continue;
            }
            let mut event_at = None;
            if index > 0 && len > 0 {
                if after_hold {
                    event_at = Some((0, Event::Release { clicks: waiting }));
                    waiting = 0;
                } else {
                    waiting = waiting.saturating_add(1);
                    event_at = Some((RELEASE_TICKS, Event::Click(waiting)));
                }
            }
            for sample in 0..len {
                let event = event_at
                    .filter(|&(at, _)| at == sample)
                    .map(|(_, event)| event);
                if matches!(event, Some(Event::Click(_))) {
                    waiting = 0;
                }
                expected.push((event, waiting == 0));
            }
        }
        expected
    }

    /// Runs the decoder over `runs` and checks every tick against the model.
    /// Returns how many ticks that was.
    fn check(runs: &[(bool, u16)]) -> usize {
        let expected = model(runs);
        let samples = runs
            .iter()
            .flat_map(|&(pressed, len)| (0..len).map(move |_| pressed));
        let mut clicks = Clicks::new();
        assert!(clicks.idle());
        let mut ticks = 0;
        for (tick, (pressed, &(event, idle))) in samples.zip(&expected).enumerate() {
            assert_eq!(clicks.tick(pressed), event, "tick {tick} of {runs:?}");
            assert_eq!(clicks.idle(), idle, "idle after tick {tick} of {runs:?}");
            ticks = tick + 1;
        }
        assert_eq!(ticks, expected.len(), "{runs:?}");
        ticks
    }

    #[test]
    fn the_model_agrees_with_the_examples() {
        let hold = |clicks, ticks| Some(Event::Hold { clicks, ticks });
        // Click, then a press held two ticks past becoming a hold, then
        // a click that counts.
        let expected = model(&[
            (false, 2),
            (true, 1),
            (false, 2),
            (true, HOLD_TICKS + 2),
            (false, 3),
            (true, 2),
            (false, RELEASE_TICKS + 2),
        ]);
        let events: Vec<(usize, Event, bool)> = expected
            .iter()
            .enumerate()
            .filter_map(|(tick, &(event, idle))| Some((tick, event?, idle)))
            .collect();
        let held = 5 + usize::from(HOLD_TICKS);
        assert_eq!(
            events,
            [
                (held, hold(1, 0).unwrap(), false),
                (held + 1, hold(1, 1).unwrap(), false),
                (held + 2, Event::Release { clicks: 1 }, true),
                (held + 7 + usize::from(RELEASE_TICKS), Event::Click(1), true),
            ]
        );
        let idle: Vec<bool> = expected.iter().map(|&(_, idle)| idle).collect();
        assert_eq!(idle[..5], [true, true, false, false, false]);
        assert!(idle[5..held + 2].iter().all(|&idle| !idle));
        assert!(idle[held + 2..held + 5].iter().all(|&idle| idle));
        assert!(
            idle[held + 5..held + 7 + usize::from(RELEASE_TICKS)]
                .iter()
                .all(|&idle| !idle)
        );
    }

    #[test]
    fn matches_the_model_for_every_six_runs_around_the_boundaries() {
        // Either side of every length that changes what a run means.
        let presses = [
            1,
            2,
            HOLD_TICKS - 1,
            HOLD_TICKS,
            HOLD_TICKS + 1,
            HOLD_TICKS + 2,
        ];
        let releases = [
            1,
            2,
            RELEASE_TICKS - 1,
            RELEASE_TICKS,
            RELEASE_TICKS + 1,
            RELEASE_TICKS + 2,
        ];
        const RUNS: usize = 6;
        let mut scripts = 0u32;
        // Each tick is checked, so every shorter script is covered as the
        // start of a longer one.
        for first in [true, false] {
            // Each run's length is one base-6 digit of `code`.
            for code in 0..6usize.pow(RUNS as u32) {
                let mut runs = [(false, 0); RUNS];
                let mut digits = code;
                for (index, run) in runs.iter_mut().enumerate() {
                    let pressed = first == index.is_multiple_of(2);
                    let lengths = if pressed { &presses } else { &releases };
                    *run = (pressed, lengths[digits % 6]);
                    digits /= 6;
                }
                check(&runs);
                scripts += 1;
            }
        }
        assert_eq!(scripts, 2 * 46_656);
    }

    #[test]
    fn matches_the_model_for_every_pattern_of_sixteen_samples() {
        // Too short to reach a hold or a counted click, so this is the
        // bounce and the short runs the lengths above leave out.
        const SAMPLES: u16 = 16;
        for pattern in 0..=u16::MAX {
            let mut runs: Vec<(bool, u16)> = Vec::new();
            for bit in 0..SAMPLES {
                let pressed = pattern >> bit & 1 == 1;
                match runs.last_mut() {
                    Some((last, len)) if *last == pressed => *len += 1,
                    _ => runs.push((pressed, 1)),
                }
            }
            assert_eq!(check(&runs), usize::from(SAMPLES));
        }
    }

    #[test]
    fn matches_the_model_for_every_two_presses_of_up_to_thirty_samples() {
        // Every length, not only the ones at the boundaries, for a press,
        // a gap and a second press, then long enough for a click to count.
        for first in 1..=HOLD_TICKS + 6 {
            for gap in 1..=RELEASE_TICKS + 6 {
                for second in 1..=HOLD_TICKS + 6 {
                    check(&[
                        (true, first),
                        (false, gap),
                        (true, second),
                        (false, RELEASE_TICKS + 2),
                    ]);
                }
            }
        }
    }
}
