use crate::ramp::{CEILING, FRACTION};

const FADE_TICKS: u16 = 16;

const _: () = assert!(
    FADE_TICKS.is_power_of_two(),
    "dividing a u16 by anything else links __mulsi3"
);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Fade {
    position: u16,
}

impl Fade {
    #[must_use]
    pub const fn new() -> Self {
        Self { position: 0 }
    }

    #[must_use]
    pub const fn position(&self) -> u16 {
        self.position
    }

    pub fn tick(&mut self, level: u8, gentle: bool) -> u16 {
        let target = u16::from(level) * FRACTION;
        let position = self.position;
        self.position = if !gentle || level == 0 || position == 0 {
            target
        } else {
            let gap = target.abs_diff(position);
            let step = (gap / FADE_TICKS).max(1).min(gap);
            if target < position {
                position.wrapping_sub(step)
            } else {
                position.wrapping_add(step)
            }
        };
        self.position
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Gradual {
    level: u8,
    waited: u8,
    down_every: u8,
    up_every: u8,
}

impl Gradual {
    #[must_use]
    pub const fn new(level: u8, down_every: u8, up_every: u8) -> Self {
        Self {
            level,
            waited: 0,
            down_every,
            up_every,
        }
    }

    #[must_use]
    pub const fn level(&self) -> u8 {
        self.level
    }

    pub const fn tick(&mut self, target: u8) -> u8 {
        let level = self.level;
        let (every, next) = if target < level {
            (self.down_every, level.saturating_sub(1))
        } else if target > level {
            (self.up_every, level.saturating_add(1))
        } else {
            self.waited = 0;
            return level;
        };
        self.waited = self.waited.saturating_add(1);
        if self.waited >= every {
            self.waited = 0;
            self.level = next;
        }
        self.level
    }

    pub const fn cap(&mut self, max: u8) {
        if self.level > max {
            self.level = max;
            self.waited = 0;
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ceiling {
    target: u8,
    level: Gradual,
}

impl Ceiling {
    #[must_use]
    pub const fn new(down_every: u8, up_every: u8) -> Self {
        Self {
            target: CEILING,
            level: Gradual::new(CEILING, down_every, up_every),
        }
    }

    #[must_use]
    pub const fn level(&self) -> u8 {
        self.level.level()
    }

    #[must_use]
    pub const fn target(&self) -> u8 {
        self.target
    }

    pub const fn tick(&mut self) -> u8 {
        self.level.tick(self.target)
    }

    pub fn set(&mut self, target: u8, level: u8) {
        if target < self.level.level() {
            self.level.cap(level.max(target));
        }
        self.target = target;
    }

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
    fn a_cap_at_the_level_its_already_at_doesnt_restart_the_wait() {
        let mut gradual = Gradual::new(100, 4, 4);
        run(&mut gradual, 0, 3);
        gradual.cap(100);
        assert_eq!(gradual.tick(0), 99, "the fourth tick still steps");
        run(&mut gradual, 0, 3);
        gradual.cap(98);
        assert_eq!(run(&mut gradual, 0, 3), 98);
        assert_eq!(gradual.tick(0), 97);
    }

    fn at(level: u8) -> u16 {
        u16::from(level) * FRACTION
    }

    fn lit(level: u8) -> Fade {
        let mut fade = Fade::new();
        fade.tick(level, false);
        fade
    }

    #[test]
    fn a_fade_starts_off_and_jumps_from_off_and_to_it_however_gentle() {
        let mut fade = Fade::new();
        assert_eq!(fade.position(), 0);
        assert_eq!(fade.tick(CEILING, true), at(CEILING));
        assert_eq!(fade.tick(0, true), 0);
        assert_eq!(fade.tick(52, true), at(52));
    }

    #[test]
    fn a_fade_that_isnt_gentle_jumps() {
        let mut fade = lit(CEILING);
        assert_eq!(fade.tick(50, false), at(50));
        fade.tick(100, true);
        assert_eq!(fade.tick(100, false), at(100));
    }

    #[test]
    fn a_gentle_fade_takes_a_quarter_second_over_a_level_a_step_at_a_time() {
        for (from, to) in [(150, 149), (149, 150), (51, 50)] {
            let mut fade = lit(from);
            for tick in 1..=FADE_TICKS {
                let now = fade.tick(to, true);
                assert_eq!(now.abs_diff(at(from)), tick, "{from} → {to}");
            }
            assert_eq!(fade.tick(to, true), at(to));
        }
    }

    #[test]
    fn a_gentle_fade_closes_in_and_never_overshoots() {
        let mut fade = lit(100);
        let mut seed = 1u32;
        for _ in 0..10_000 {
            seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            let level = 1 + (seed >> 16) as u8 % CEILING;
            for _ in 0..=(seed >> 24) % 40 {
                let before = fade.position();
                let after = fade.tick(level, true);
                let target = at(level);
                assert!(after.abs_diff(target) < before.abs_diff(target) || after == target);
                assert!((before < target) == (after < target) || after == target);
            }
        }
    }

    #[test]
    fn a_gentle_fade_trails_a_level_every_two_ticks_by_eight_levels_at_most() {
        let mut fade = lit(CEILING);
        let mut worst = 0;
        for level in (50..CEILING).rev() {
            for _ in 0..2 {
                worst = worst.max(fade.tick(level, true) - at(level));
            }
        }
        assert_eq!(worst, at(8));
        let mut ticks = 0;
        while fade.position() != at(50) {
            fade.tick(50, true);
            ticks += 1;
            assert!(ticks < 2 * 64, "still at {}", fade.position());
        }
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
        ceiling.set(30, 10);
        assert_eq!(ceiling.level(), 30);
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
