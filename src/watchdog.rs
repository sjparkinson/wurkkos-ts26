//! When to feed the watchdog.
//!
//! The firmware runs the WDT in window mode, as AN2632 §3.2 recommends. A
//! `wdr` in the closed window after the last one resets the chip, and so does
//! no `wdr` by the end of the open window that follows it. So a loop that's
//! stuck fails, and so does a jump into the wrong code that happens to land
//! on the feed.
//!
//! The WDT and the 64 Hz tick both run off the ultra-low-power oscillator, so
//! a tick is always 16 WDT clocks however far that oscillator drifts, and the
//! window can be sized in ticks. Feeding every tick would land on a window
//! edge whatever the setting, so the loop feeds every third tick. That's 48
//! clocks, halfway through a 32-clock open window that starts 32 clocks after
//! the last feed.
//!
//! Window mode only starts with the first `wdr` (DS40002204A §19.3.3.2). Until
//! then the WDT most likely times out after the open window alone (the
//! datasheet implies it rather than saying so), so the first pass feeds it
//! straight away, which also lines the windows up with the ticks.
//!
//! That's all there is. The TS21 also proves its watchdog at every power-on
//! with three deliberate resets and a marker in `.noinit`. This firmware
//! doesn't, by choice: that TS21 went dark straight after a verified flash of
//! that stack, and a firmware hang wasn't ruled out. So nobody's seen this
//! watchdog bite on a real chip until docs/bench-plan.md says so.
//!
//! The watchdog can't catch its own oscillator stopping, since the tick stops
//! with it. [`TICK_DEADLINE_CYCLES`] is for that, and only with the light on.

use crate::TICKS_PER_SECOND;
use crate::ramp::PWM_TOP;

/// Cycles of the WDT's 1.024 kHz clock in one tick.
pub const CLOCKS_PER_TICK: u16 = 1024 / TICKS_PER_SECOND;
/// The closed window after each feed, in WDT clocks. `hw::start_watchdog` sets
/// `WDT.CTRLA.WINDOW` to match.
pub const CLOSED_CLOCKS: u16 = 32;
/// The open window after the closed one, in WDT clocks. `hw::start_watchdog` sets
/// `WDT.CTRLA.PERIOD` to match.
pub const OPEN_CLOCKS: u16 = 32;
/// How many ticks apart the feeds are.
pub const FEED_TICKS: u8 = 3;

// A feed lands strictly inside the open window, and the first one comes
// before the open window alone runs out, since the first tick arrives within
// a tick of `hw::start_watchdog` turning the WDT on. `wdr` takes two to three WDT clocks
// to sync (§19.3.6), the same for every feed, so it shifts the windows without
// changing the gap between feeds.
const _: () = {
    let gap = FEED_TICKS as u16 * CLOCKS_PER_TICK;
    assert!(gap > CLOSED_CLOCKS && gap < CLOSED_CLOCKS + OPEN_CLOCKS);
    assert!(CLOCKS_PER_TICK < OPEN_CLOCKS);
};

/// The main clock in hertz: the 20 MHz oscillator divided by 2, which is what
/// `hw::init` sets. xtask's simulator holds the firmware to the same number.
pub const CLOCK_HZ: u32 = 10_000_000;

/// How many PWM cycles `hw::wait_for_tick` waits for a tick with the light on
/// before it gives up, and `main` turns the light off for good (`hw::halt`).
///
/// The tick and the watchdog both run off the 32 kHz oscillator. If that
/// stopped with the light on, the loop would wait for a tick that never came
/// and the watchdog would never bite, so the boost converter would stay on
/// with nothing watching the cell or the temperature. While the light's on,
/// TCA0's overflow wakes that wait once a PWM cycle, and TCA0 runs off the
/// 20 MHz oscillator. So the wait counts those.
///
/// A PWM cycle is 2 × [`PWM_TOP`] clocks, 51 µs, and a tick is about 306 of
/// them. 2,048 is about 104 ms, over six ticks.
///
/// A tick that's only late is the watchdog's to catch. It bites 64 WDT clocks
/// after a feed has synced, which takes up to three more (§19.3.6): 67 in
/// all, a little over four ticks however slow the oscillator runs. At worst
/// the 32 kHz oscillator is 30% slow and the 20 MHz one 5% fast (DS40002204A
/// tables 36-13 and 36-12, over the full operating range, from
/// characterisation and not production test), which makes a tick about 460
/// cycles and those 67 clocks about 1,925. The deadline is longer, by 6%, so
/// with both oscillators inside their limits the watchdog gets there first,
/// and this is left for an oscillator that's stopped. Nobody's seen either on
/// a real chip.
pub const TICK_DEADLINE_CYCLES: u16 = 2048;

const _: () = {
    // Main clocks in a PWM cycle, in a tick and in the deadline.
    let cycle = 2 * PWM_TOP as u32;
    let tick = CLOCK_HZ / TICKS_PER_SECOND as u32;
    let deadline = TICK_DEADLINE_CYCLES as u32 * cycle;
    // About a tenth of a second.
    assert!(deadline > 6 * tick && deadline < 7 * tick);
    // The slowest tick inside the datasheet's limits: 5% more clocks from the
    // 20 MHz oscillator, in the 1 / 0.7 of the time the 32 kHz one takes.
    let slowest = tick * 105 / 70;
    // The longest the watchdog takes to bite, in WDT clocks from the `wdr`:
    // three to sync it, then both windows. The deadline counts from the wait
    // after a tick, which is never before the last feed.
    let bite = (3 + CLOSED_CLOCKS + OPEN_CLOCKS) as u32;
    assert!(deadline * CLOCKS_PER_TICK as u32 > bite * slowest);
};

/// Counts ticks between feeds.
#[derive(Clone, Copy, Debug, Default)]
pub struct Feeder {
    /// Ticks until the next feed.
    wait: u8,
    /// Whether the last tick fed it.
    fed: bool,
}

impl Feeder {
    pub const fn new() -> Self {
        Self {
            wait: 0,
            fed: false,
        }
    }

    /// Call once a tick. True when this pass feeds the watchdog, which is the
    /// first one and then every [`FEED_TICKS`]th.
    pub fn tick(&mut self) -> bool {
        self.fed = if self.wait == 0 {
            self.wait = FEED_TICKS - 1;
            true
        } else {
            self.wait -= 1;
            false
        };
        self.fed
    }

    /// Whether the last [`tick`](Self::tick) fed the watchdog.
    pub const fn fed(&self) -> bool {
        self.fed
    }

    /// Whether the sleep after this pass can be power-down, `resting` being
    /// whether the torch has anything to do. Not straight after a feed: a
    /// `wdr` takes two or three WDT clocks to sync (§19.3.6), and the
    /// datasheet doesn't say it gets there with the peripheral clock stopped.
    /// If it didn't, the watchdog would reset a resting torch. That costs an
    /// idle tick in [`FEED_TICKS`] while resting.
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

    /// The figures in [`TICK_DEADLINE_CYCLES`]'s comment.
    #[test]
    fn the_tick_deadline_is_104_ms_against_ticks_of_306_cycles_or_460_at_worst() {
        let cycle = 2 * u32::from(PWM_TOP);
        let tick = CLOCK_HZ / u32::from(TICKS_PER_SECOND);
        assert_eq!(tick / cycle, 306);
        assert_eq!((tick * 105 / 70).div_ceil(cycle), 460);
        // 67 WDT clocks at that speed, 16 to a tick.
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
