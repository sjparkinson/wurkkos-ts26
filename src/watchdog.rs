//! When to feed the watchdog.
//!
//! The WDT runs in window mode (AN2632 §3.2) off the same oscillator as the
//! tick, so a tick is always 16 WDT clocks and the window is sized in ticks.
//! Feeding every tick would land on a window edge, so the loop feeds every
//! third. Window mode only starts with the first `wdr` (DS40002204A
//! §19.3.3.2), and until then the WDT most likely times out after the open
//! window alone (implied, not stated), so the first pass feeds straight away.
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

// The first tick arrives within a tick of `hw::start_watchdog`. `wdr` takes
// two to three WDT clocks to sync (§19.3.6), the same for every feed.
const _: () = {
    let gap = u16_from(FEED_TICKS) * CLOCKS_PER_TICK;
    assert!(gap > CLOSED_CLOCKS && gap < CLOSED_CLOCKS + OPEN_CLOCKS);
    assert!(CLOCKS_PER_TICK < OPEN_CLOCKS);
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

#[derive(Clone, Copy, Debug, Default)]
pub struct Feeder {
    wait: u8,
    fed: bool,
}

impl Feeder {
    pub const fn new() -> Self {
        Self {
            wait: 0,
            fed: false,
        }
    }

    pub const fn tick(&mut self) -> bool {
        self.fed = match self.wait.checked_sub(1) {
            None => {
                self.wait = FEED_TICKS - 1;
                true
            }
            Some(wait) => {
                self.wait = wait;
                false
            }
        };
        self.fed
    }

    pub const fn fed(&self) -> bool {
        self.fed
    }

    /// Not straight after a feed, since the datasheet doesn't say a `wdr`
    /// finishes syncing (§19.3.6) with the peripheral clock stopped.
    pub const fn power_down(&self, resting: bool) -> bool {
        resting && !self.fed
    }
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

    #[test]
    fn feeds_on_the_first_tick() {
        assert!(Feeder::new().tick());
    }

    #[test]
    fn feeds_every_third_tick_after_that() {
        let mut feeder = Feeder::new();
        let feeds: [bool; 10] = core::array::from_fn(|_| feeder.tick());
        let (t, f) = (true, false);
        assert_eq!(feeds, [t, f, f, t, f, f, t, f, f, t]);
    }

    #[test]
    fn remembers_whether_the_last_tick_fed_it() {
        let mut feeder = Feeder::new();
        assert!(!feeder.fed(), "nothing's fed it before the first tick");
        for _ in 0..10 {
            let fed = feeder.tick();
            assert_eq!(feeder.fed(), fed);
        }
    }

    #[test]
    fn never_powers_down_straight_after_a_feed() {
        let mut feeder = Feeder::new();
        assert!(feeder.power_down(true), "before the first feed");
        let after: [bool; 6] = core::array::from_fn(|_| {
            feeder.tick();
            feeder.power_down(true)
        });
        let (t, f) = (true, false);
        assert_eq!(after, [f, t, t, f, t, t]);
    }

    #[test]
    fn never_powers_down_with_something_to_do() {
        let mut feeder = Feeder::new();
        for _ in 0..FEED_TICKS * 2 {
            assert!(!feeder.power_down(false));
            feeder.tick();
        }
    }

    #[test]
    fn every_feed_lands_mid_window() {
        let middle = CLOSED_CLOCKS + OPEN_CLOCKS / 2;
        let mut feeder = Feeder::new();
        let mut last = None;
        for tick in 0..1000u16 {
            if feeder.tick() {
                if let Some(last) = last {
                    assert_eq!((tick - last) * CLOCKS_PER_TICK, middle, "tick {tick}");
                }
                last = Some(tick);
            }
        }
        assert!(last.is_some());
    }
}
