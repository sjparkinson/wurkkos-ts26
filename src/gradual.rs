//! A level that follows a target one step at a time, so a step-down fades
//! rather than jumps.
//!
//! It moves at most one level per tick, and you pick how many ticks each step
//! waits for, separately going down and going up. A [`Ceiling`] is one with
//! somewhere to head for, and thermal regulation (`src/thermal.rs`) and
//! low-voltage protection (`src/battery.rs`) keep one each. UI changes (on,
//! off, ramping) shouldn't go through it: they want to be instant.

use crate::ramp::CEILING;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Gradual {
    level: u8,
    waited: u8,
    down_every: u8,
    up_every: u8,
}

impl Gradual {
    /// Starts at `level`, stepping down once every `down_every` ticks and up
    /// once every `up_every`. A rate of `0` counts as `1`, one level a tick.
    pub const fn new(level: u8, down_every: u8, up_every: u8) -> Self {
        Self {
            level,
            waited: 0,
            down_every,
            up_every,
        }
    }

    pub fn level(&self) -> u8 {
        self.level
    }

    /// One tick towards `target`. Returns the new level, which is never more
    /// than one away from the last.
    pub fn tick(&mut self, target: u8) -> u8 {
        if target == self.level {
            self.waited = 0;
            return self.level;
        }
        let every = if target < self.level {
            self.down_every
        } else {
            self.up_every
        };
        self.waited = self.waited.saturating_add(1);
        if self.waited >= every {
            self.waited = 0;
            if target < self.level {
                self.level -= 1;
            } else {
                self.level += 1;
            }
        }
        self.level
    }

    /// Drops the level to `max` straight away if it's above it. Only do this
    /// when nothing can see the difference, such as pulling a ceiling down to
    /// the level the light is already at.
    pub fn cap(&mut self, max: u8) {
        if self.level > max {
            self.level = max;
            self.waited = 0;
        }
    }
}

/// A limit on the level that fades to wherever it's sent, starting at
/// [`CEILING`], which is no limit at all.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ceiling {
    /// Where it's heading.
    target: u8,
    level: Gradual,
}

impl Ceiling {
    /// Fading down a level every `down_every` ticks and up every `up_every`.
    pub const fn new(down_every: u8, up_every: u8) -> Self {
        Self {
            target: CEILING,
            level: Gradual::new(CEILING, down_every, up_every),
        }
    }

    /// The ceiling right now.
    pub fn level(&self) -> u8 {
        self.level.level()
    }

    /// Where it's heading.
    pub fn target(&self) -> u8 {
        self.target
    }

    /// One tick: moves a step towards the target and returns the ceiling.
    pub fn tick(&mut self) -> u8 {
        self.level.tick(self.target)
    }

    /// Heads for `target`, with the light at `level` (`0` for off). A
    /// ceiling above the light's level does nothing, so one that's coming
    /// down is first pulled down to the level, or to `target` if that's
    /// higher. Nothing visible changes, and the fade shows from its first
    /// step.
    pub fn set(&mut self, target: u8, level: u8) {
        if target < self.level.level() {
            self.level.cap(level.max(target));
        }
        self.target = target;
    }

    /// Heads a level higher, up to [`CEILING`].
    pub fn raise(&mut self) {
        self.target = self.target.saturating_add(1).min(CEILING);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(gradual: &mut Gradual, target: u8, ticks: u16) -> u8 {
        for _ in 0..ticks {
            gradual.tick(target);
        }
        gradual.level()
    }

    #[test]
    fn stays_put_at_the_target() {
        let mut gradual = Gradual::new(80, 1, 1);
        assert_eq!(run(&mut gradual, 80, 1000), 80);
    }

    #[test]
    fn moves_one_level_a_tick_at_full_speed() {
        let mut gradual = Gradual::new(150, 1, 1);
        for expected in (50..150).rev() {
            assert_eq!(gradual.tick(50), expected);
        }
        assert_eq!(run(&mut gradual, 50, 10), 50);
        for expected in 51..=150 {
            assert_eq!(gradual.tick(150), expected);
        }
    }

    #[test]
    fn a_rate_of_zero_is_one_level_a_tick() {
        let mut gradual = Gradual::new(10, 0, 0);
        assert_eq!(gradual.tick(0), 9);
        assert_eq!(gradual.tick(20), 10);
    }

    #[test]
    fn waits_its_turn_in_each_direction() {
        let mut gradual = Gradual::new(100, 2, 8);
        assert_eq!(run(&mut gradual, 0, 20), 90);
        assert_eq!(run(&mut gradual, 200, 80), 100);
    }

    #[test]
    fn never_moves_more_than_one_level_a_tick() {
        let mut gradual = Gradual::new(0, 1, 3);
        let mut seed = 1u32;
        for _ in 0..10_000 {
            seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            let target = (seed >> 16) as u8;
            let before = gradual.level();
            let after = gradual.tick(target);
            assert!(before.abs_diff(after) <= 1, "{before} → {after}");
            assert!(
                after.abs_diff(target) <= before.abs_diff(target),
                "{before} → {after} moving to {target}"
            );
        }
    }

    #[test]
    fn a_new_target_mid_step_doesnt_skip_the_wait() {
        let mut gradual = Gradual::new(100, 4, 4);
        run(&mut gradual, 0, 3);
        assert_eq!(gradual.level(), 100);
        assert_eq!(gradual.tick(50), 99);
    }

    #[test]
    fn cap_only_ever_lowers() {
        let mut gradual = Gradual::new(150, 1, 1);
        gradual.cap(60);
        assert_eq!(gradual.level(), 60);
        gradual.cap(100);
        assert_eq!(gradual.level(), 60);
    }

    #[test]
    fn a_ceiling_starts_at_no_limit_and_fades() {
        let mut ceiling = Ceiling::new(1, 1);
        assert_eq!(ceiling.level(), CEILING);
        ceiling.set(CEILING - 10, CEILING);
        assert_eq!(ceiling.target(), CEILING - 10);
        assert_eq!(ceiling.tick(), CEILING - 1);
    }

    #[test]
    fn a_ceiling_coming_down_drops_to_the_light_first() {
        let mut ceiling = Ceiling::new(1, 1);
        ceiling.set(40, 60);
        assert_eq!(ceiling.level(), 60);
        // Not below the target, though.
        ceiling.set(30, 10);
        assert_eq!(ceiling.level(), 30);
        // And going up, it stays where it is.
        ceiling.set(80, 10);
        assert_eq!(ceiling.level(), 30);
    }

    #[test]
    fn a_ceiling_raises_a_level_at_a_time_to_no_limit() {
        let mut ceiling = Ceiling::new(1, 1);
        ceiling.set(CEILING - 2, CEILING);
        ceiling.raise();
        assert_eq!(ceiling.target(), CEILING - 1);
        ceiling.raise();
        ceiling.raise();
        assert_eq!(ceiling.target(), CEILING);
        assert_eq!(ceiling.level(), CEILING);
    }
}
