use ts26::button::{HOLD_TICKS, RELEASE_TICKS};
use ts26::testing::Rng;

/// (pressed, ticks) spans that alternate, starting released.
#[derive(Clone, Debug)]
pub struct RandomSwitching {
    rng: Rng,
    pressed: bool,
}

impl RandomSwitching {
    const EDGES: [u16; 10] = [
        1,
        2,
        4,
        HOLD_TICKS - 1,
        HOLD_TICKS,
        HOLD_TICKS + 1,
        HOLD_TICKS + 2,
        RELEASE_TICKS - 1,
        RELEASE_TICKS,
        RELEASE_TICKS + 1,
    ];

    pub const fn new(seed: u32) -> Self {
        Self {
            rng: Rng::new(seed),
            pressed: true,
        }
    }
}

impl Iterator for RandomSwitching {
    type Item = (bool, u16);

    fn next(&mut self) -> Option<Self::Item> {
        self.pressed = !self.pressed;
        let ticks = match self.rng.next_u32() % 3 {
            0 => 1 + (self.rng.next_u32() % 100) as u16,
            _ => Self::EDGES[self.rng.next_u32() as usize % Self::EDGES.len()],
        };
        Some((self.pressed, ticks))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ts26::button::{Clicks, Event};

    type Runs = [(bool, u32)];

    /// Runs must alternate between pressed and released.
    fn model(runs: &Runs) -> Vec<(u32, Event)> {
        let saturate = |ticks: u32| ticks.min(u32::from(u16::MAX)) as u16;
        let mut out = Vec::new();
        let (mut t, mut count) = (0, 0u8);
        let mut press = None;
        for &(pressed, len) in runs {
            if pressed {
                for i in u32::from(HOLD_TICKS)..len {
                    let ticks = saturate(i) - HOLD_TICKS;
                    out.push((
                        t + i,
                        Event::Hold {
                            clicks: count,
                            ticks,
                        },
                    ));
                }
                press = Some(len);
            } else {
                match press.take() {
                    Some(len) if saturate(len) > HOLD_TICKS => {
                        out.push((t, Event::Release { clicks: count }));
                        count = 0;
                    }
                    Some(_) => count = count.saturating_add(1),
                    None => {}
                }
                if count > 0 && len > u32::from(RELEASE_TICKS) {
                    out.push((t + u32::from(RELEASE_TICKS), Event::Click(count)));
                    count = 0;
                }
            }
            t += len;
        }
        out
    }

    fn actual(runs: &Runs) -> Vec<(u32, Event)> {
        let mut clicks = Clicks::new();
        let samples = runs
            .iter()
            .flat_map(|&(pressed, len)| std::iter::repeat_n(pressed, len as usize));
        (0..)
            .zip(samples)
            .filter_map(|(t, pressed)| clicks.tick(pressed).map(|event| (t, event)))
            .collect()
    }

    #[test]
    fn clicks_matches_the_model_on_random_switching() {
        for seed in 1..=500 {
            let runs: Vec<_> = RandomSwitching::new(seed)
                .take(60)
                .map(|(pressed, ticks)| (pressed, u32::from(ticks)))
                .collect();
            assert_eq!(actual(&runs), model(&runs), "seed {seed}: {runs:?}");
        }
    }

    #[test]
    fn random_switching_alternates_from_released() {
        let spans: Vec<_> = RandomSwitching::new(0).take(1000).collect();
        assert!(spans.iter().step_by(2).all(|&(pressed, _)| !pressed));
        assert!(spans.iter().skip(1).step_by(2).all(|&(pressed, _)| pressed));
        assert!(spans.iter().all(|&(_, ticks)| (1..=100).contains(&ticks)));
    }

    #[test]
    fn a_click_still_counts_after_a_long_wait() {
        let runs = [(false, 70_000), (true, 4), (false, 30)];
        assert_eq!(actual(&runs), model(&runs));
        assert_eq!(
            actual(&runs).last(),
            Some(&(70_004 + u32::from(RELEASE_TICKS), Event::Click(1)))
        );
    }

    #[test]
    fn a_very_long_hold_saturates_and_still_releases() {
        let runs = [(true, 70_000), (false, 2)];
        let events = actual(&runs);
        assert_eq!(events, model(&runs));
        let top = Event::Hold {
            clicks: 0,
            ticks: u16::MAX - HOLD_TICKS,
        };
        let last: Vec<_> = events.iter().rev().take(2).map(|&(_, e)| e).collect();
        assert_eq!(last, [Event::Release { clicks: 0 }, top]);
    }

    #[test]
    fn hundreds_of_clicks_count_up_to_255() {
        let mut runs: Vec<_> = (0..300).flat_map(|_| [(true, 2), (false, 2)]).collect();
        *runs.last_mut().unwrap() = (false, 30);
        assert_eq!(actual(&runs), model(&runs));
        assert_eq!(
            actual(&runs),
            [(299 * 4 + 2 + u32::from(RELEASE_TICKS), Event::Click(255))]
        );
    }
}
