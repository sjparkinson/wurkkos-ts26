//! When to feed the watchdog, and when the tick slows to 1 Hz at rest.
//!
//! Awake, the WDT runs in window mode (AN2632 §3.2) off the same oscillator as
//! the tick, so a tick is always 16 WDT clocks and the window is sized in
//! ticks. Feeding every tick would land on a window edge, so the loop feeds
//! every third. Window mode only starts with the first `wdr` (DS40002204A
//! §19.3.3.2), and until then the WDT most likely times out after the open
//! window alone (implied, not stated), so the first pass feeds straight away.
//!
//! At rest the PIT ticks once a second, the WDT times out after 8 s with no
//! window, and a press wakes the chip. Whether a write to WDT.CTRLA restarts
//! its count isn't in the datasheet, and the PIT's first tick after a change
//! of period can land anywhere in it (§23.5.2.2), so each change is ordered to
//! be safe either way, as the asserts below work through.
//!
//! Nothing proves the watchdog bites at power-on, by choice, since an earlier
//! torch went dark straight after a verified flash of a stack that did, and a
//! firmware hang wasn't ruled out. Unconfirmed on a real chip.

use crate::ramp::PWM_TOP;
use crate::{TICKS_PER_SECOND, u16_from, u32_from};

pub const CLOCKS_PER_TICK: u16 = 1024 / TICKS_PER_SECOND;
/// `hw::start_watchdog` sets `WDT.CTRLA.WINDOW` to match.
pub const CLOSED_CLOCKS: u16 = 32;
/// `hw::start_watchdog` sets `WDT.CTRLA.PERIOD` to match.
pub const OPEN_CLOCKS: u16 = 32;
pub const FEED_TICKS: u8 = 3;
/// At rest, with no window. `hw::slow_down` sets `WDT.CTRLA.PERIOD` to match.
pub const REST_CLOCKS: u16 = 8192;
/// 64 Hz ticks in one at rest. `hw::slow_down` sets `RTC.PITCTRLA.PERIOD` to
/// match.
pub const REST_TICKS: u16 = TICKS_PER_SECOND;

/// Two to three WDT clocks, and a `wdr` meanwhile is ignored (§19.3.6).
const WDR_SYNC_CLOCKS: u16 = 3;
/// The datasheet gives no time for a WDT.CTRLA write to sync, so this takes
/// the `wdr`'s. Unconfirmed.
const CTRLA_SYNC_CLOCKS: u16 = 3;
/// From a tick to the end of its pass. The simulator's longest is under 1,300
/// instructions, at most about half a WDT clock at 10 MHz.
const PASS_CLOCKS: u16 = 2;

// The first tick arrives within a tick of `hw::start_watchdog`. `wdr` takes
// two to three WDT clocks to sync (§19.3.6), the same for every feed.
const _: () = {
    let gap = u16_from(FEED_TICKS) * CLOCKS_PER_TICK;
    assert!(gap > CLOSED_CLOCKS && gap < CLOSED_CLOCKS + OPEN_CLOCKS);
    assert!(CLOCKS_PER_TICK < OPEN_CLOCKS);
};

// Each change of pace, counted from the last `wdr` that took.
const _: () = {
    let tick = CLOCKS_PER_TICK;
    // `Step::Slow` comes the tick after a feed and has no `wdr`, so the window
    // only has to last until the write lands.
    assert!(tick + PASS_CLOCKS + CTRLA_SYNC_CLOCKS < CLOSED_CLOCKS + OPEN_CLOCKS);
    // The longest at rest without a `wdr`: from the feed before `Step::Slow`, a
    // rest tick up to a whole one late that goes `Step::Fast`, then the feed on
    // the first fast tick, up to a fast one late.
    let rest = REST_TICKS * tick;
    assert!(tick + rest + tick + 3 * PASS_CLOCKS < REST_CLOCKS);
    // `Step::Window` lands a tick after the `wdr` before it, or after a rest
    // feed that made that one ignored, with no window yet.
    assert!(tick + WDR_SYNC_CLOCKS + PASS_CLOCKS + CTRLA_SYNC_CLOCKS < OPEN_CLOCKS);
    // Its own `wdr` waits for the write to land, so it's the first in window
    // mode, which no closed window comes before, and a fast tick after the
    // last, since `hw::speed_up` drops a 1 Hz tick that came during its sync.
    assert!(tick > WDR_SYNC_CLOCKS + PASS_CLOCKS);
    // The next feed comes `FEED_TICKS` on, less what the write's sync took.
    let gap = u16_from(FEED_TICKS) * tick;
    assert!(gap - PASS_CLOCKS - CTRLA_SYNC_CLOCKS > CLOSED_CLOCKS + WDR_SYNC_CLOCKS);
    assert!(gap + PASS_CLOCKS + WDR_SYNC_CLOCKS < CLOSED_CLOCKS + OPEN_CLOCKS);
};

/// `hw::init` sets this clock, and the simulator holds the firmware to it.
pub const CLOCK_HZ: u32 = 10_000_000;

/// For a stopped 32 kHz oscillator with the light on, which stops the tick and
/// the watchdog together. TCA0 runs off the 20 MHz oscillator, so the wait
/// counts its cycles instead.
///
/// A tick that's only late is the watchdog's to catch, within 67 WDT clocks of
/// a feed (§19.3.6). With the 32 kHz oscillator 30% slow and the 20 MHz one 5%
/// fast (DS40002204A tables 36-13 and 36-12, from characterisation), that's
/// about 1,925 cycles, so the watchdog gets there first. Unconfirmed on a real
/// chip.
pub const TICK_DEADLINE_CYCLES: u16 = 2048;

const _: () = {
    let cycle = 2 * u32_from(PWM_TOP);
    let tick = CLOCK_HZ / u32_from(TICKS_PER_SECOND);
    let deadline = u32_from(TICK_DEADLINE_CYCLES) * cycle;
    assert!(deadline > 6 * tick && deadline < 7 * tick);
    let slowest = tick * 105 / 70;
    // Three to sync the `wdr`, then both windows. The deadline counts from
    // the wait after a tick, which is never before the last feed.
    let bite = u32_from(3 + CLOSED_CLOCKS + OPEN_CLOCKS);
    assert!(deadline * u32_from(CLOCKS_PER_TICK) > bite * slowest);
};

/// What the loop does with the watchdog and the tick after a pass.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Feed,
    /// To 1 Hz: the watchdog to [`REST_CLOCKS`] with no window, then the PIT,
    /// then PA5's sense on.
    Slow,
    /// To 64 Hz: PA5's sense off, then the PIT. The watchdog stays at rest.
    Fast,
    /// The window back on, then a `wdr`.
    Window,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    /// With the window, feeding when `wait` runs out.
    Awake {
        wait: u8,
    },
    Resting,
    /// The first 64 Hz tick after [`Step::Fast`], which feeds at rest's timeout.
    Waking,
    /// The second, which puts the window back.
    Arming,
}

#[derive(Clone, Copy, Debug)]
pub struct Pace {
    state: State,
    /// [`Step::Window`]'s `wdr` came on the tick just gone.
    armed: bool,
}

impl Default for Pace {
    fn default() -> Self {
        Self::new()
    }
}

impl Pace {
    pub const fn new() -> Self {
        Self {
            state: State::Awake { wait: 0 },
            armed: false,
        }
    }

    /// Whether the sleep after a resting pass may be power-down. Not after
    /// [`Step::Window`], since nothing says its `wdr` syncs with CLK_PER
    /// stopped, and one that synced on waking a tick later would leave the
    /// next feed in the closed window.
    pub const fn powers_down(&self) -> bool {
        !self.armed
    }

    pub const fn slow(&self) -> bool {
        matches!(self.state, State::Resting)
    }

    pub const fn windowed(&self) -> bool {
        matches!(self.state, State::Awake { .. })
    }

    /// What the tick that's just come is worth in 64 Hz ones.
    pub const fn ticks(&self) -> u16 {
        if self.slow() { REST_TICKS } else { 1 }
    }

    /// The switch woke the chip at rest, rather than a tick.
    pub const fn woken(&mut self) -> Option<Step> {
        self.state = State::Waking;
        Some(Step::Fast)
    }

    /// After a tick's work. `rest` is `Torch::slow`.
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
        let wait = (1u16..)
            .find(|_| pace.tick(false) == Some(Step::Feed))
            .unwrap();
        assert_eq!(
            wait * CLOCKS_PER_TICK,
            CLOSED_CLOCKS + OPEN_CLOCKS / 2,
            "ticks after Window"
        );
    }
}
