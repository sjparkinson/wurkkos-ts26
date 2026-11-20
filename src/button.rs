use crate::TICKS_PER_SECOND;

pub const HOLD_TICKS: u16 = TICKS_PER_SECOND * 3 / 8;

pub const RELEASE_TICKS: u16 = TICKS_PER_SECOND * 7 / 32;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    Click(u8),
    Hold { clicks: u8, ticks: u16 },
    Release { clicks: u8 },
}

#[derive(Clone, Debug, Default)]
pub struct Debounce {
    pressed: bool,
    last: bool,
}

impl Debounce {
    pub const fn new() -> Self {
        Self {
            pressed: false,
            last: false,
        }
    }

    pub const fn settled(&self) -> bool {
        !self.pressed && !self.last
    }

    pub const fn sample(&mut self, pressed: bool) -> bool {
        if pressed == self.last {
            self.pressed = pressed;
        }
        self.last = pressed;
        self.pressed
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Gesture {
    #[default]
    Idle,
    Down {
        clicks: u8,
        ticks: u16,
    },
    Up {
        clicks: u8,
        ticks: u16,
    },
    Spent {
        up: u16,
    },
}

#[derive(Clone, Debug, Default)]
pub struct Clicks {
    gesture: Gesture,
}

impl Clicks {
    pub const fn new() -> Self {
        Self {
            gesture: Gesture::Idle,
        }
    }

    pub const fn idle(&self) -> bool {
        matches!(self.gesture, Gesture::Idle)
    }

    pub const fn counting(&self) -> bool {
        matches!(self.gesture, Gesture::Down { .. } | Gesture::Up { .. })
    }

    pub const fn pending(&self) -> Option<Event> {
        match self.gesture {
            Gesture::Down { clicks, .. } => Some(Event::Hold { clicks, ticks: 0 }),
            Gesture::Up { clicks, .. } => Some(Event::Click(clicks)),
            Gesture::Idle | Gesture::Spent { .. } => None,
        }
    }

    pub fn tick(&mut self, pressed: bool) -> Option<Event> {
        self.tick_up_to(pressed, None)
    }

    pub fn tick_up_to(&mut self, pressed: bool, most: Option<u8>) -> Option<Event> {
        match (&mut self.gesture, pressed) {
            (Gesture::Idle, false) => None,
            (Gesture::Idle, true) => {
                self.gesture = Gesture::Down {
                    clicks: 0,
                    ticks: 0,
                };
                None
            }
            (Gesture::Down { clicks, ticks }, true) => {
                *ticks = ticks.saturating_add(1);
                let clicks = *clicks;
                ticks
                    .checked_sub(HOLD_TICKS)
                    .map(|ticks| Event::Hold { clicks, ticks })
            }
            (&mut Gesture::Down { clicks, ticks }, false) if ticks >= HOLD_TICKS => {
                self.gesture = Gesture::Idle;
                Some(Event::Release { clicks })
            }
            (&mut Gesture::Down { clicks, .. }, false) => {
                let clicks = clicks.saturating_add(1);
                if most.is_some_and(|most| clicks >= most) {
                    self.gesture = Gesture::Spent { up: 1 };
                    Some(Event::Click(clicks))
                } else {
                    self.gesture = Gesture::Up { clicks, ticks: 1 };
                    None
                }
            }
            (&mut Gesture::Up { clicks, .. }, true) => {
                self.gesture = Gesture::Down { clicks, ticks: 0 };
                None
            }
            (&mut Gesture::Up { clicks, ticks }, false) if ticks >= RELEASE_TICKS => {
                self.gesture = Gesture::Idle;
                Some(Event::Click(clicks))
            }
            (Gesture::Up { ticks, .. }, false) => {
                *ticks = ticks.saturating_add(1);
                None
            }
            (Gesture::Spent { up }, true) => {
                *up = 0;
                None
            }
            (&mut Gesture::Spent { up }, false) if up >= RELEASE_TICKS => {
                self.gesture = Gesture::Idle;
                None
            }
            (Gesture::Spent { up }, false) => {
                *up = up.saturating_add(1);
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    extern crate std;

    use super::*;
    use std::vec::Vec;

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
    fn a_change_of_the_switch_counts_on_the_second_sample_that_shows_it() {
        let mut switch = Debounce::new();
        assert!(switch.settled());
        assert!(!switch.sample(true));
        assert!(!switch.settled(), "a press that may count");
        assert!(switch.sample(true));
        assert!(switch.sample(false), "one sample up");
        assert!(!switch.sample(false));
        assert!(switch.settled());
    }

    #[test]
    fn the_switch_only_follows_two_samples_that_agree_for_every_pattern_of_sixteen() {
        for pattern in 0..=u16::MAX {
            let mut switch = Debounce::new();
            let (mut last, mut was) = (false, false);
            for bit in 0..16 {
                let sample = pattern >> bit & 1 == 1;
                let now = switch.sample(sample);
                let expected = if sample == last { sample } else { was };
                assert_eq!(now, expected, "sample {bit} of {pattern:#018b}");
                assert_eq!(switch.settled(), !now && !sample);
                (last, was) = (sample, now);
            }
        }
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
        assert_eq!(TICKS_PER_SECOND, 64);
        assert_eq!(HOLD_TICKS, 24);
        assert_eq!(
            u32::from(HOLD_TICKS) * 1000 / u32::from(TICKS_PER_SECOND),
            375
        );
    }

    #[test]
    fn clicks_count_after_the_switch_is_left_alone_for_about_220_ms() {
        assert_eq!(RELEASE_TICKS, 14);
        assert_eq!(
            u32::from(RELEASE_TICKS) * 1000 / u32::from(TICKS_PER_SECOND),
            218
        );
    }

    #[test]
    fn the_click_that_reaches_most_counts_as_it_is_let_go() {
        for most in 1..=3 {
            let mut clicks = Clicks::new();
            for n in 1..most {
                assert_eq!(clicks.tick_up_to(true, Some(most)), None);
                assert_eq!(
                    clicks.tick_up_to(false, Some(most)),
                    None,
                    "click {n} of {most}"
                );
            }
            assert_eq!(clicks.tick_up_to(true, Some(most)), None);
            assert_eq!(
                clicks.tick_up_to(false, Some(most)),
                Some(Event::Click(most))
            );
            assert!(!clicks.idle() && !clicks.counting());
            for _ in 0..2 * RELEASE_TICKS {
                assert_eq!(clicks.tick_up_to(false, Some(most)), None);
            }
            assert!(clicks.idle());
        }
    }

    #[test]
    fn nothing_counts_after_the_click_that_reached_most_until_the_switch_is_left_alone() {
        for pattern in 0..=u16::MAX {
            let mut clicks = Clicks::new();
            clicks.tick_up_to(true, Some(1));
            assert_eq!(clicks.tick_up_to(false, Some(1)), Some(Event::Click(1)));
            let mut up = 1;
            for bit in 0..16 {
                if up > RELEASE_TICKS {
                    break;
                }
                let pressed = pattern >> bit & 1 == 1;
                assert_eq!(
                    clicks.tick_up_to(pressed, Some(1)),
                    None,
                    "sample {bit} of {pattern:#018b}"
                );
                up = if pressed { 0 } else { up + 1 };
            }
        }
    }

    #[test]
    fn a_hold_straight_after_the_click_that_reached_most_does_nothing() {
        let mut clicks = Clicks::new();
        clicks.tick_up_to(true, Some(1));
        assert_eq!(clicks.tick_up_to(false, Some(1)), Some(Event::Click(1)));
        for _ in 0..3 * HOLD_TICKS {
            assert_eq!(clicks.tick_up_to(true, Some(3)), None);
        }
        for _ in 0..=RELEASE_TICKS {
            assert_eq!(clicks.tick_up_to(false, Some(3)), None);
        }
        for _ in 0..HOLD_TICKS {
            assert_eq!(clicks.tick_up_to(true, Some(3)), None);
        }
        assert_eq!(
            clicks.tick_up_to(true, Some(3)),
            Some(Event::Hold {
                clicks: 0,
                ticks: 0
            })
        );
    }

    #[test]
    fn a_click_counts_again_once_the_switch_has_been_up_for_the_release_timeout() {
        for (up, counts) in [(RELEASE_TICKS, false), (RELEASE_TICKS + 1, true)] {
            let mut clicks = Clicks::new();
            clicks.tick_up_to(true, Some(1));
            for _ in 0..up {
                clicks.tick_up_to(false, Some(1));
            }
            clicks.tick_up_to(true, Some(1));
            assert_eq!(
                clicks.tick_up_to(false, Some(1)),
                counts.then_some(Event::Click(1)),
                "{up} samples up"
            );
        }
    }

    #[test]
    fn a_hold_is_a_hold_whatever_most_is() {
        let mut clicks = Clicks::new();
        for _ in 0..HOLD_TICKS {
            assert_eq!(clicks.tick_up_to(true, Some(1)), None);
        }
        assert_eq!(
            clicks.tick_up_to(true, Some(1)),
            Some(Event::Hold {
                clicks: 0,
                ticks: 0
            })
        );
        assert_eq!(
            clicks.tick_up_to(false, Some(1)),
            Some(Event::Release { clicks: 0 })
        );
    }

    #[test]
    fn a_press_let_go_just_before_it_becomes_a_hold_is_a_click() {
        let (events, n) = run(&[(true, HOLD_TICKS), (false, RELEASE_TICKS + 1)]);
        assert_eq!(n, 1);
        assert_eq!(events[0], Some(Event::Click(1)));
    }

    #[test]
    fn a_press_let_go_just_after_it_becomes_a_hold_is_a_hold() {
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

    type Expected = (Option<Event>, bool);

    fn model(runs: &[(bool, u16)], most: Option<u8>) -> Vec<Expected> {
        let mut expected = Vec::new();
        let mut waiting = 0u8;
        let mut after_hold = false;
        let mut spent = false;
        for (index, &(pressed, len)) in runs.iter().enumerate() {
            if pressed {
                for sample in 0..len {
                    let event = (!spent && sample >= HOLD_TICKS).then(|| Event::Hold {
                        clicks: waiting,
                        ticks: sample - HOLD_TICKS,
                    });
                    expected.push((event, false));
                }
                after_hold = len > HOLD_TICKS;
                continue;
            }
            let mut event_at = None;
            if index > 0 && len > 0 && !spent {
                if after_hold {
                    event_at = Some((0, Event::Release { clicks: waiting }));
                    waiting = 0;
                } else {
                    waiting = waiting.saturating_add(1);
                    spent = most.is_some_and(|most| waiting >= most);
                    let at = if spent { 0 } else { RELEASE_TICKS };
                    event_at = Some((at, Event::Click(waiting)));
                }
            }
            for sample in 0..len {
                let event = event_at
                    .filter(|&(at, _)| at == sample)
                    .map(|(_, event)| event);
                if matches!(event, Some(Event::Click(_))) {
                    waiting = 0;
                }
                spent &= sample < RELEASE_TICKS;
                expected.push((event, waiting == 0 && !spent));
            }
        }
        expected
    }

    const MOSTS: [Option<u8>; 4] = [None, Some(1), Some(2), Some(3)];

    fn check(runs: &[(bool, u16)], most: Option<u8>) -> usize {
        let expected = model(runs, most);
        let samples = runs
            .iter()
            .flat_map(|&(pressed, len)| (0..len).map(move |_| pressed));
        let mut clicks = Clicks::new();
        assert!(clicks.idle());
        let mut ticks = 0;
        for (tick, (pressed, &(event, idle))) in samples.zip(&expected).enumerate() {
            assert_eq!(
                clicks.tick_up_to(pressed, most),
                event,
                "tick {tick} of {runs:?} up to {most:?}"
            );
            assert_eq!(
                clicks.idle(),
                idle,
                "idle after tick {tick} of {runs:?} up to {most:?}"
            );
            ticks = tick + 1;
        }
        assert_eq!(ticks, expected.len(), "{runs:?}");
        ticks
    }

    #[test]
    fn the_model_agrees_with_the_examples() {
        let spent = model(
            &[
                (true, 2),
                (false, 3),
                (true, HOLD_TICKS + 2),
                (false, RELEASE_TICKS + 2),
            ],
            Some(1),
        );
        let mut expected = std::vec![(None, false); spent.len()];
        expected[2].0 = Some(Event::Click(1));
        expected[spent.len() - 2..].fill((None, true));
        assert_eq!(spent, expected);

        let hold = |clicks, ticks| Some(Event::Hold { clicks, ticks });
        let expected = model(
            &[
                (false, 2),
                (true, 1),
                (false, 2),
                (true, HOLD_TICKS + 2),
                (false, 3),
                (true, 2),
                (false, RELEASE_TICKS + 2),
            ],
            None,
        );
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
        for first in [true, false] {
            for code in 0..6usize.pow(RUNS as u32) {
                let mut runs = [(false, 0); RUNS];
                let mut digits = code;
                for (index, run) in runs.iter_mut().enumerate() {
                    let pressed = first == index.is_multiple_of(2);
                    let lengths = if pressed { &presses } else { &releases };
                    *run = (pressed, lengths[digits % 6]);
                    digits /= 6;
                }
                for most in MOSTS {
                    check(&runs, most);
                }
                scripts += 1;
            }
        }
        assert_eq!(scripts, 2 * 46_656);
    }

    #[test]
    fn matches_the_model_for_every_pattern_of_sixteen_samples() {
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
            for most in MOSTS {
                assert_eq!(check(&runs, most), usize::from(SAMPLES));
            }
        }
    }

    #[test]
    fn matches_the_model_for_every_two_presses_of_up_to_thirty_samples() {
        for first in 1..=HOLD_TICKS + 6 {
            for gap in 1..=RELEASE_TICKS + 6 {
                for second in 1..=HOLD_TICKS + 6 {
                    for most in MOSTS {
                        check(
                            &[
                                (true, first),
                                (false, gap),
                                (true, second),
                                (false, RELEASE_TICKS + 2),
                            ],
                            most,
                        );
                    }
                }
            }
        }
    }
}
