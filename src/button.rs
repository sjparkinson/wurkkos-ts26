use crate::TICKS_PER_SECOND;

pub const HOLD_TICKS: u16 = TICKS_PER_SECOND * 3 / 8;

pub const RELEASE_TICKS: u16 = TICKS_PER_SECOND * 7 / 32;

/// A press this long is `pending` as a hold. Unconfirmed that it leaves the filter time to fall.
pub const CLICK_TICKS: u16 = HOLD_TICKS / 2;

/// After a click that's sent as it's let go, how long the switch has to be up
/// before a press counts. Unmeasured.
pub const GUARD_TICKS: u16 = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Count {
    Each,
    Group,
}

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
    #[must_use]
    pub const fn new() -> Self {
        Self {
            pressed: false,
            last: false,
        }
    }

    #[must_use]
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
        sent: bool,
    },
    Bounce,
}

#[derive(Clone, Debug, Default)]
pub struct Clicks {
    gesture: Gesture,
}

impl Clicks {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            gesture: Gesture::Idle,
        }
    }

    #[must_use]
    pub const fn idle(&self) -> bool {
        matches!(self.gesture, Gesture::Idle)
    }

    #[must_use]
    pub const fn counting(&self) -> bool {
        match self.gesture {
            Gesture::Idle | Gesture::Bounce => false,
            Gesture::Down { .. } => true,
            Gesture::Up { sent, .. } => !sent,
        }
    }

    #[must_use]
    pub const fn pending(&self, count: Count) -> Option<Event> {
        match self.gesture {
            Gesture::Down { clicks, ticks } if ticks < CLICK_TICKS => {
                Some(Event::Click(match count {
                    Count::Each => 1,
                    Count::Group => clicks.saturating_add(1),
                }))
            }
            Gesture::Down { clicks, .. } => Some(Event::Hold { clicks, ticks: 0 }),
            Gesture::Up {
                clicks,
                sent: false,
                ..
            } => Some(Event::Click(clicks)),
            Gesture::Up { sent: true, .. } | Gesture::Bounce | Gesture::Idle => None,
        }
    }

    pub fn tick(&mut self, pressed: bool) -> Option<Event> {
        self.tick_counting(pressed, Count::Group)
    }

    pub fn tick_counting(&mut self, pressed: bool, count: Count) -> Option<Event> {
        match (&mut self.gesture, pressed) {
            (Gesture::Idle, false) | (Gesture::Bounce, true) => None,
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
                let sent = count == Count::Each;
                self.gesture = Gesture::Up {
                    clicks: if sent { 1 } else { clicks.saturating_add(1) },
                    ticks: 1,
                    sent,
                };
                sent.then_some(Event::Click(1))
            }
            (&mut Gesture::Up { ticks, sent, .. }, true) if sent && ticks < GUARD_TICKS => {
                self.gesture = Gesture::Bounce;
                None
            }
            (Gesture::Bounce, false) => {
                self.gesture = Gesture::Up {
                    clicks: 1,
                    ticks: 1,
                    sent: true,
                };
                None
            }
            (&mut Gesture::Up { clicks, .. }, true) => {
                self.gesture = Gesture::Down { clicks, ticks: 0 };
                None
            }
            (
                &mut Gesture::Up {
                    clicks,
                    ticks,
                    sent,
                },
                false,
            ) if ticks >= RELEASE_TICKS => {
                self.gesture = Gesture::Idle;
                (!sent).then_some(Event::Click(clicks))
            }
            (Gesture::Up { ticks, .. }, false) => {
                *ticks = ticks.saturating_add(1);
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

    fn run_counting(script: &[(bool, u16)], count: Count) -> ([Option<Event>; 64], usize) {
        let mut clicks = Clicks::new();
        let mut events = [None; 64];
        let mut n = 0;
        for &(pressed, ticks) in script {
            for _ in 0..ticks {
                if let Some(event) = clicks.tick_counting(pressed, count) {
                    if n < events.len() {
                        events[n] = Some(event);
                    }
                    n += 1;
                }
            }
        }
        (events, n)
    }

    fn run(script: &[(bool, u16)]) -> ([Option<Event>; 64], usize) {
        run_counting(script, Count::Group)
    }

    #[expect(
        clippy::unnecessary_wraps,
        reason = "it stands beside what `tick` returns, which is an `Option`"
    )]
    const fn hold(clicks: u8, ticks: u16) -> Option<Event> {
        Some(Event::Hold { clicks, ticks })
    }

    const COUNTS: [Count; 2] = [Count::Group, Count::Each];

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
        assert_eq!(events[..n], [Some(Event::Click(1))]);
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
        assert_eq!(events[..n], [Some(Event::Click(3))]);
    }

    #[test]
    fn nothing_until_the_release_timeout() {
        let (_, n) = run(&[(true, 4), (false, RELEASE_TICKS - 1)]);
        assert_eq!(n, 0);
    }

    #[test]
    fn hold_then_release() {
        let (events, n) = run(&[(true, HOLD_TICKS + 4), (false, RELEASE_TICKS + 1)]);
        assert_eq!(
            events[..n],
            [
                hold(0, 0),
                hold(0, 1),
                hold(0, 2),
                hold(0, 3),
                Some(Event::Release { clicks: 0 })
            ]
        );
    }

    #[test]
    fn click_then_hold() {
        let (events, n) = run(&[(true, 4), (false, 6), (true, HOLD_TICKS + 2), (false, 1)]);
        assert_eq!(
            events[..n],
            [hold(1, 0), hold(1, 1), Some(Event::Release { clicks: 1 })]
        );
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
    fn each_click_counts_as_it_is_let_go() {
        let mut clicks = Clicks::new();
        for _ in 0..3 {
            for _ in 0..4 {
                assert_eq!(clicks.tick_counting(true, Count::Each), None);
            }
            assert_eq!(
                clicks.tick_counting(false, Count::Each),
                Some(Event::Click(1))
            );
            assert!(!clicks.idle() && !clicks.counting());
            assert_eq!(clicks.pending(Count::Each), None);
            for _ in 1..GUARD_TICKS {
                assert_eq!(clicks.tick_counting(false, Count::Each), None);
            }
        }
        for _ in GUARD_TICKS..=RELEASE_TICKS {
            assert_eq!(clicks.tick_counting(false, Count::Each), None);
        }
        assert!(clicks.idle());
    }

    #[test]
    fn a_hold_straight_after_a_click_counts_the_click() {
        let (events, n) = run_counting(
            &[(true, 4), (false, 6), (true, HOLD_TICKS + 2), (false, 1)],
            Count::Each,
        );
        assert_eq!(
            events[..n],
            [
                Some(Event::Click(1)),
                hold(1, 0),
                hold(1, 1),
                Some(Event::Release { clicks: 1 })
            ]
        );
    }

    #[test]
    fn a_hold_once_the_click_is_over_counts_none() {
        let (events, n) = run_counting(
            &[
                (true, 4),
                (false, RELEASE_TICKS + 1),
                (true, HOLD_TICKS + 1),
                (false, 1),
            ],
            Count::Each,
        );
        assert_eq!(
            events[..n],
            [
                Some(Event::Click(1)),
                hold(0, 0),
                Some(Event::Release { clicks: 0 })
            ]
        );
    }

    #[test]
    fn after_a_click_a_press_counts_once_the_switch_has_been_up_for_the_guard() {
        for (up, counts) in [(GUARD_TICKS - 1, false), (GUARD_TICKS, true)] {
            let script = [
                (true, 4),
                (false, up),
                (true, 1),
                (false, RELEASE_TICKS + 1),
            ];
            assert_eq!(
                run_counting(&script, Count::Each).1,
                if counts { 2 } else { 1 },
                "{up} up"
            );
        }
    }

    #[test]
    fn a_press_that_starts_inside_the_guard_counts_for_nothing_however_long() {
        let (events, n) = run_counting(
            &[
                (true, 4),
                (false, 1),
                (true, 3 * HOLD_TICKS),
                (false, GUARD_TICKS),
                (true, 4),
                (false, RELEASE_TICKS + 1),
            ],
            Count::Each,
        );
        assert_eq!(events[..n], [Some(Event::Click(1)), Some(Event::Click(1))]);
    }

    #[test]
    fn a_hold_is_a_hold_whichever_way_clicks_count() {
        for count in COUNTS {
            let (events, n) = run_counting(&[(true, HOLD_TICKS + 1), (false, 1)], count);
            assert_eq!(
                events[..n],
                [hold(0, 0), Some(Event::Release { clicks: 0 })],
                "{count:?}"
            );
        }
    }

    #[test]
    fn a_press_is_pending_as_a_click_until_half_way_to_a_hold_and_as_a_hold_after() {
        for count in COUNTS {
            let mut clicks = Clicks::new();
            assert_eq!(clicks.pending(count), None);
            for tick in 0..HOLD_TICKS {
                clicks.tick_counting(true, count);
                let pending = if tick < CLICK_TICKS {
                    Some(Event::Click(1))
                } else {
                    hold(0, 0)
                };
                assert_eq!(clicks.pending(count), pending, "tick {tick}, {count:?}");
            }
        }
    }

    #[test]
    fn a_press_after_a_click_is_pending_as_another_click_or_a_2h() {
        let mut clicks = Clicks::new();
        clicks.tick_counting(true, Count::Each);
        assert_eq!(
            clicks.tick_counting(false, Count::Each),
            Some(Event::Click(1))
        );
        for _ in 1..GUARD_TICKS {
            clicks.tick_counting(false, Count::Each);
        }
        for tick in 0..HOLD_TICKS {
            clicks.tick_counting(true, Count::Each);
            let pending = if tick < CLICK_TICKS {
                Some(Event::Click(1))
            } else {
                hold(1, 0)
            };
            assert_eq!(clicks.pending(Count::Each), pending, "tick {tick}");
        }
    }

    #[test]
    fn a_press_inside_the_guard_is_pending_as_nothing() {
        let mut clicks = Clicks::new();
        clicks.tick_counting(true, Count::Each);
        clicks.tick_counting(false, Count::Each);
        for tick in 0..2 * HOLD_TICKS {
            clicks.tick_counting(true, Count::Each);
            assert_eq!(clicks.pending(Count::Each), None, "tick {tick}");
        }
    }

    #[test]
    fn grouped_clicks_are_pending_as_the_count_they_would_make() {
        let mut clicks = Clicks::new();
        for n in 1..=3 {
            clicks.tick_counting(true, Count::Group);
            assert_eq!(clicks.pending(Count::Group), Some(Event::Click(n)));
            assert_eq!(clicks.tick_counting(false, Count::Group), None);
            assert_eq!(clicks.pending(Count::Group), Some(Event::Click(n)));
        }
        for _ in 1..RELEASE_TICKS {
            assert_eq!(clicks.tick_counting(false, Count::Group), None);
        }
        assert_eq!(
            clicks.tick_counting(false, Count::Group),
            Some(Event::Click(3))
        );
        assert_eq!(clicks.pending(Count::Group), None);
    }

    #[test]
    fn a_press_let_go_just_before_it_becomes_a_hold_is_a_click() {
        let (events, n) = run(&[(true, HOLD_TICKS), (false, RELEASE_TICKS + 1)]);
        assert_eq!(events[..n], [Some(Event::Click(1))]);
    }

    #[test]
    fn a_press_let_go_just_after_it_becomes_a_hold_is_a_hold() {
        let (events, n) = run(&[(true, HOLD_TICKS + 1), (false, RELEASE_TICKS + 1)]);
        assert_eq!(
            events[..n],
            [hold(0, 0), Some(Event::Release { clicks: 0 })]
        );
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
        assert_eq!(clicks.tick(true), hold(255, 0));
        assert_eq!(clicks.tick(false), Some(Event::Release { clicks: 255 }));
        assert!(clicks.idle());
    }

    type Expected = (Option<Event>, bool);

    #[derive(Clone, Copy)]
    enum Phase {
        Idle,
        Down { clicks: u8, held: u16 },
        Up { clicks: u8, up: u16, sent: bool },
        Bounce,
    }

    fn model(runs: &[(bool, u16)], count: Count) -> Vec<Expected> {
        let mut expected = Vec::new();
        let mut phase = Phase::Idle;
        for &(pressed, len) in runs {
            if pressed {
                let clicks = match phase {
                    Phase::Up { up, sent: true, .. } if up < GUARD_TICKS => {
                        expected.extend((0..len).map(|_| (None, false)));
                        phase = Phase::Bounce;
                        continue;
                    }
                    Phase::Up { clicks, .. } => clicks,
                    Phase::Idle | Phase::Down { .. } | Phase::Bounce => 0,
                };
                for sample in 0..len {
                    let event = (sample >= HOLD_TICKS).then(|| Event::Hold {
                        clicks,
                        ticks: sample - HOLD_TICKS,
                    });
                    expected.push((event, false));
                }
                phase = Phase::Down { clicks, held: len };
                continue;
            }
            let (clicks, up, sent) =
                match phase {
                    Phase::Idle => {
                        expected.extend((0..len).map(|_| (None, true)));
                        continue;
                    }
                    Phase::Down { clicks, held } if held > HOLD_TICKS => {
                        expected.extend((0..len).map(|sample| {
                            ((sample == 0).then_some(Event::Release { clicks }), true)
                        }));
                        phase = Phase::Idle;
                        continue;
                    }
                    Phase::Down { .. } if count == Count::Each => (1, 0, true),
                    Phase::Bounce => (1, 0, true),
                    Phase::Down { clicks, .. } => (clicks.saturating_add(1), 0, false),
                    Phase::Up { clicks, up, sent } => (clicks, up, sent),
                };
            let let_go = matches!(phase, Phase::Down { .. });
            let over = RELEASE_TICKS.saturating_sub(up);
            for sample in 0..len {
                let event = if let_go && sample == 0 && sent {
                    Some(Event::Click(1))
                } else if sample == over && !sent {
                    Some(Event::Click(clicks))
                } else {
                    None
                };
                expected.push((event, sample >= over));
            }
            phase = if len > over {
                Phase::Idle
            } else {
                Phase::Up {
                    clicks,
                    up: up + len,
                    sent,
                }
            };
        }
        expected
    }

    fn check(runs: &[(bool, u16)], count: Count) -> usize {
        let expected = model(runs, count);
        let samples = runs
            .iter()
            .flat_map(|&(pressed, len)| (0..len).map(move |_| pressed));
        let mut clicks = Clicks::new();
        assert!(clicks.idle());
        let mut ticks = 0;
        for (tick, (pressed, &(event, idle))) in samples.zip(&expected).enumerate() {
            assert_eq!(
                clicks.tick_counting(pressed, count),
                event,
                "tick {tick} of {runs:?} counting {count:?}"
            );
            assert_eq!(
                clicks.idle(),
                idle,
                "idle after tick {tick} of {runs:?} counting {count:?}"
            );
            ticks = tick + 1;
        }
        assert_eq!(ticks, expected.len(), "{runs:?}");
        ticks
    }

    #[test]
    fn the_model_agrees_with_the_examples() {
        let each = model(
            &[
                (true, 2),
                (false, 3),
                (true, HOLD_TICKS + 2),
                (false, RELEASE_TICKS + 2),
            ],
            Count::Each,
        );
        let held = 5 + usize::from(HOLD_TICKS);
        let mut expected = std::vec![(None, false); each.len()];
        expected[2].0 = Some(Event::Click(1));
        expected[held].0 = hold(1, 0);
        expected[held + 1].0 = hold(1, 1);
        expected[held + 2..].fill((None, true));
        expected[held + 2].0 = Some(Event::Release { clicks: 1 });
        assert_eq!(each, expected);

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
            Count::Group,
        );
        let events: Vec<(usize, Event, bool)> = expected
            .iter()
            .enumerate()
            .filter_map(|(tick, &(event, idle))| Some((tick, event?, idle)))
            .collect();
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
        const RUNS: usize = 6;
        let press_lengths = [
            1,
            2,
            HOLD_TICKS - 1,
            HOLD_TICKS,
            HOLD_TICKS + 1,
            HOLD_TICKS + 2,
        ];
        let release_lengths = [
            1,
            2,
            RELEASE_TICKS - 1,
            RELEASE_TICKS,
            RELEASE_TICKS + 1,
            RELEASE_TICKS + 2,
        ];
        let mut scripts = 0u32;
        for first in [true, false] {
            for code in 0..6usize.pow(RUNS as u32) {
                let mut runs = [(false, 0); RUNS];
                let mut digits = code;
                for (index, run) in runs.iter_mut().enumerate() {
                    let pressed = first == index.is_multiple_of(2);
                    let lengths = if pressed {
                        &press_lengths
                    } else {
                        &release_lengths
                    };
                    *run = (pressed, lengths[digits % 6]);
                    digits /= 6;
                }
                for count in COUNTS {
                    check(&runs, count);
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
            for count in COUNTS {
                assert_eq!(check(&runs, count), usize::from(SAMPLES));
            }
        }
    }

    #[test]
    fn matches_the_model_for_every_two_presses_of_up_to_thirty_samples() {
        for first in 1..=HOLD_TICKS + 6 {
            for gap in 1..=RELEASE_TICKS + 6 {
                for second in 1..=HOLD_TICKS + 6 {
                    for count in COUNTS {
                        check(
                            &[
                                (true, first),
                                (false, gap),
                                (true, second),
                                (false, RELEASE_TICKS + 2),
                            ],
                            count,
                        );
                    }
                }
            }
        }
    }
}
