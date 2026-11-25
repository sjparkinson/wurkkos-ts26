use crate::ramp::PWM_TOP;
use crate::{TICKS_PER_SECOND, u16_from, u32_from};

pub const CLOCKS_PER_TICK: u16 = 1024 / TICKS_PER_SECOND;
pub const CLOSED_CLOCKS: u16 = 32;
pub const OPEN_CLOCKS: u16 = 32;
pub const FEED_TICKS: u8 = 3;
pub const REST_CLOCKS: u16 = 8192;
pub const REST_TICKS: u16 = TICKS_PER_SECOND;

// DS40002204A §19.3.6: two to three WDT clocks, and a `wdr` meanwhile is ignored.
const WDR_SYNC_CLOCKS: u16 = 3;
// Unconfirmed: the datasheet gives no figure, so this takes the `wdr`'s.
const CTRLA_SYNC_CLOCKS: u16 = 3;
pub const PASS_CLOCKS: u16 = 2;

const _: () = {
    let gap = u16_from(FEED_TICKS) * CLOCKS_PER_TICK;
    assert!(
        gap > CLOSED_CLOCKS && gap < CLOSED_CLOCKS + OPEN_CLOCKS,
        "a feed has to land in the open window"
    );
    assert!(
        CLOCKS_PER_TICK < OPEN_CLOCKS,
        "the first feed comes a tick after start-up, with only the open window to beat"
    );
};

const _: () = {
    let tick = CLOCKS_PER_TICK;
    assert!(
        tick + PASS_CLOCKS + CTRLA_SYNC_CLOCKS < CLOSED_CLOCKS + OPEN_CLOCKS,
        "Step::Slow comes a tick after a feed with no wdr, so the window has to last until \
         its write lands"
    );
    let rest = REST_TICKS * tick;
    assert!(
        tick + rest + tick + 3 * PASS_CLOCKS < REST_CLOCKS,
        "at rest a wdr can be a whole rest tick late, then a fast one late on waking"
    );
    assert!(
        tick + WDR_SYNC_CLOCKS + PASS_CLOCKS + CTRLA_SYNC_CLOCKS < OPEN_CLOCKS,
        "Step::Window's write lands a tick after a wdr, with no window yet"
    );
    assert!(
        tick > WDR_SYNC_CLOCKS + PASS_CLOCKS,
        "Step::Window's wdr comes a fast tick after the last one, which mustn't still be syncing"
    );
    let gap = u16_from(FEED_TICKS) * tick;
    assert!(
        gap - PASS_CLOCKS - CTRLA_SYNC_CLOCKS > CLOSED_CLOCKS + WDR_SYNC_CLOCKS
            && gap + PASS_CLOCKS + WDR_SYNC_CLOCKS < CLOSED_CLOCKS + OPEN_CLOCKS,
        "the feed after Step::Window has to land in the open window"
    );
};

pub const CLOCK_HZ: u32 = 10_000_000;

pub const TICK_DEADLINE_CYCLES: u16 = 2048;

const _: () = {
    let cycle = 2 * u32_from(PWM_TOP);
    let tick = CLOCK_HZ / u32_from(TICKS_PER_SECOND);
    let deadline = u32_from(TICK_DEADLINE_CYCLES) * cycle;
    assert!(deadline > 6 * tick && deadline < 7 * tick);
    // DS40002204A tables 36-12 and 36-13: the 20 MHz oscillator 5% fast, the 32 kHz one 30% slow.
    let slowest = tick * 105 / 70;
    let bite = u32_from(WDR_SYNC_CLOCKS + CLOSED_CLOCKS + OPEN_CLOCKS);
    assert!(
        deadline * u32_from(CLOCKS_PER_TICK) > bite * slowest,
        "a tick that's only late is the watchdog's to catch, so its bite has to beat the deadline"
    );
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Feed,
    Slow,
    Fast,
    Window,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Awake { wait: u8 },
    Resting,
    Waking,
    Arming,
}

#[derive(Clone, Copy, Debug)]
pub struct Pace {
    state: State,
    armed: bool,
}

impl Default for Pace {
    fn default() -> Self {
        Self::new()
    }
}

impl Pace {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            state: State::Awake { wait: 0 },
            armed: false,
        }
    }

    #[must_use]
    pub const fn powers_down(&self) -> bool {
        !self.armed
    }

    #[must_use]
    pub const fn slow(&self) -> bool {
        matches!(self.state, State::Resting)
    }

    #[must_use]
    pub const fn windowed(&self) -> bool {
        matches!(self.state, State::Awake { .. })
    }

    #[must_use]
    pub const fn ticks(&self) -> u16 {
        if self.slow() { REST_TICKS } else { 1 }
    }

    pub const fn woken(&mut self) -> Option<Step> {
        self.state = State::Waking;
        Some(Step::Fast)
    }

    pub const fn tick(&mut self, rest: bool) -> Option<Step> {
        let (state, step) = match self.state {
            State::Resting if rest => (State::Resting, Some(Step::Feed)),
            State::Resting => (State::Waking, Some(Step::Fast)),
            State::Waking => (State::Arming, Some(Step::Feed)),
            State::Arming => (Self::FED, Some(Step::Window)),
            Self::FED if rest => (State::Resting, Some(Step::Slow)),
            State::Awake { wait } => match wait.checked_sub(1) {
                None => (Self::FED, Some(Step::Feed)),
                Some(wait) => (State::Awake { wait }, None),
            },
        };
        self.state = state;
        self.armed = matches!(step, Some(Step::Window));
        step
    }

    const FED: State = State::Awake {
        wait: FEED_TICKS - 1,
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tick_is_16_wdt_clocks() {
        assert_eq!(CLOCKS_PER_TICK, 16);
    }

    #[test]
    fn the_tick_deadline_is_104_ms_against_ticks_of_306_cycles_or_460_at_worst() {
        let cycle = 2 * u32::from(PWM_TOP);
        let tick = CLOCK_HZ / u32::from(TICKS_PER_SECOND);
        assert_eq!(tick / cycle, 306);
        assert_eq!((tick * 105 / 70).div_ceil(cycle), 460);
        assert_eq!((67 * (tick * 105 / 70) / 16).div_ceil(cycle), 1925);
        let deadline = u32::from(TICK_DEADLINE_CYCLES) * cycle;
        assert_eq!(deadline / (CLOCK_HZ / 1000), 104);
    }

    fn awake<const N: usize>(pace: &mut Pace) -> [Option<Step>; N] {
        core::array::from_fn(|_| pace.tick(false))
    }

    #[test]
    fn feeds_on_the_first_tick() {
        assert_eq!(Pace::new().tick(false), Some(Step::Feed));
    }

    #[test]
    fn feeds_every_third_tick_after_that() {
        let feeds = awake::<10>(&mut Pace::new()).map(|step| step == Some(Step::Feed));
        let (t, f) = (true, false);
        assert_eq!(feeds, [t, f, f, t, f, f, t, f, f, t]);
    }

    #[test]
    fn every_feed_lands_mid_window() {
        let middle = CLOSED_CLOCKS + OPEN_CLOCKS / 2;
        let mut pace = Pace::new();
        let mut last = None;
        for tick in 0..1000u16 {
            if pace.tick(false) == Some(Step::Feed) {
                if let Some(last) = last {
                    assert_eq!((tick - last) * CLOCKS_PER_TICK, middle, "tick {tick}");
                }
                last = Some(tick);
            }
        }
        assert!(last.is_some());
    }

    #[test]
    fn slows_only_on_the_tick_after_a_feed() {
        let mut pace = Pace::new();
        assert_eq!(pace.tick(false), Some(Step::Feed));
        assert_eq!(pace.tick(false), None);
        assert_eq!(pace.tick(true), None, "two ticks after the feed");
        assert_eq!(pace.tick(true), Some(Step::Feed));
        assert_eq!(pace.tick(true), Some(Step::Slow));
        assert!(pace.slow());
        assert!(!pace.windowed());
    }

    fn resting() -> Pace {
        let mut pace = Pace::new();
        pace.tick(false);
        assert_eq!(pace.tick(true), Some(Step::Slow));
        pace
    }

    #[test]
    fn feeds_every_tick_at_rest_and_each_is_worth_a_second() {
        let mut pace = resting();
        for _ in 0..10 {
            assert_eq!(pace.ticks(), TICKS_PER_SECOND);
            assert_eq!(pace.tick(true), Some(Step::Feed));
        }
    }

    #[test]
    fn something_to_do_at_rest_feeds_then_puts_the_window_back_two_ticks_later() {
        let mut pace = resting();
        assert_eq!(pace.tick(false), Some(Step::Fast));
        assert_eq!(pace.ticks(), 1);
        assert_eq!(pace.tick(true), Some(Step::Feed), "rest's timeout");
        assert!(!pace.windowed());
        assert_eq!(pace.tick(true), Some(Step::Window));
        assert!(pace.windowed());
        assert_eq!(pace.tick(true), Some(Step::Slow), "the tick after a feed");
    }

    #[test]
    fn a_press_at_rest_does_the_same_from_its_first_tick() {
        let mut pace = resting();
        assert_eq!(pace.woken(), Some(Step::Fast));
        assert!(!pace.slow());
        assert_eq!(pace.ticks(), 1);
        let steps = awake::<8>(&mut pace);
        let (feed, window) = (Some(Step::Feed), Some(Step::Window));
        assert_eq!(steps, [feed, window, None, None, feed, None, None, feed]);
    }

    #[test]
    fn idles_only_on_the_tick_after_the_window_comes_back() {
        let mut pace = resting();
        pace.woken();
        for (step, powers_down) in [
            (Some(Step::Feed), true),
            (Some(Step::Window), false),
            (Some(Step::Slow), true),
        ] {
            assert_eq!(pace.tick(true), step);
            assert_eq!(pace.powers_down(), powers_down, "{step:?}");
        }
    }

    #[test]
    fn the_feed_after_the_window_comes_back_lands_mid_window() {
        let mut pace = resting();
        pace.woken();
        pace.tick(false);
        assert_eq!(pace.tick(false), Some(Step::Window));
        let wait = (1..=u16::MAX)
            .find(|_| pace.tick(false) == Some(Step::Feed))
            .unwrap();
        assert_eq!(
            wait * CLOCKS_PER_TICK,
            CLOSED_CLOCKS + OPEN_CLOCKS / 2,
            "ticks after Window"
        );
    }
}
