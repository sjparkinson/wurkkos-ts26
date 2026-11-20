#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Channel {
    Voltage,
    Ground,
    Temperature,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Convert {
    pub channel: Channel,
    pub setup: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reading {
    Voltage(u16),
    Ground(u16),
    Temperature(u16),
}

pub const FULL_SCALE: u16 = 4 * 1023;

/// Likely: 3 LSb a sample, DS40002204A table 36-27's typical error, which gives no maximum.
pub const GROUND_MAX: u16 = 4 * 3;

pub const CYCLE_TICKS: u8 = 16;
/// Unconfirmed: that ground settles in one thrown-away conversion.
const GROUND_AT: u8 = CYCLE_TICKS / 4;
const TEMPERATURE_AT: u8 = CYCLE_TICKS / 2;

const _: () = assert!(CYCLE_TICKS.is_power_of_two());

#[derive(Clone, Debug, Default)]
pub struct Schedule {
    phase: u8,
    temperature: bool,
}

impl Schedule {
    pub const fn new() -> Self {
        Self {
            phase: 0,
            temperature: false,
        }
    }

    pub const fn rest(&mut self) {
        *self = Self::new();
    }

    pub fn tick(
        &mut self,
        result: Option<u16>,
        temperature: bool,
    ) -> (Option<Reading>, Option<Convert>) {
        let phase = self.phase;
        self.phase = phase.wrapping_add(1) & (CYCLE_TICKS - 1);
        let (channel, start) = match phase {
            TEMPERATURE_AT.. => (Channel::Temperature, TEMPERATURE_AT),
            GROUND_AT.. => (Channel::Ground, GROUND_AT),
            _ => (Channel::Voltage, 0),
        };
        let step = phase.wrapping_sub(start);
        if step == 0 && channel == Channel::Temperature {
            self.temperature = temperature;
        }
        if channel == Channel::Temperature && !self.temperature {
            return (None, None);
        }
        match step {
            0 | 1 => (
                None,
                Some(Convert {
                    channel,
                    setup: step == 0,
                }),
            ),
            2 => {
                let reading = match channel {
                    Channel::Voltage => Reading::Voltage(result.unwrap_or(0)),
                    Channel::Ground => Reading::Ground(result.unwrap_or(u16::MAX)),
                    Channel::Temperature => Reading::Temperature(result.unwrap_or(0)),
                };
                (Some(reading), None)
            }
            _ => (None, None),
        }
    }
}

#[cfg(test)]
mod tests {
    extern crate std;

    use super::*;
    use std::vec::Vec;

    fn run(
        schedule: &mut Schedule,
        ticks: usize,
        result: Option<u16>,
        temperature: bool,
    ) -> Vec<(Option<Reading>, Option<Convert>)> {
        (0..ticks)
            .map(|_| schedule.tick(result, temperature))
            .collect()
    }

    const fn convert(channel: Channel, setup: bool) -> Option<Convert> {
        Some(Convert { channel, setup })
    }

    #[test]
    fn reads_the_voltage_then_ground_4_ticks_later_then_the_temperature() {
        let mut schedule = Schedule::new();
        let ticks = run(&mut schedule, 16, Some(7), true);
        let mut expected = [(None, None); 16];
        expected[0].1 = convert(Channel::Voltage, true);
        expected[1].1 = convert(Channel::Voltage, false);
        expected[2].0 = Some(Reading::Voltage(7));
        expected[4].1 = convert(Channel::Ground, true);
        expected[5].1 = convert(Channel::Ground, false);
        expected[6].0 = Some(Reading::Ground(7));
        expected[8].1 = convert(Channel::Temperature, true);
        expected[9].1 = convert(Channel::Temperature, false);
        expected[10].0 = Some(Reading::Temperature(7));
        assert_eq!(ticks, expected);
    }

    #[test]
    fn each_channel_comes_round_every_16_ticks() {
        let mut schedule = Schedule::new();
        let ticks = run(&mut schedule, 160, Some(1), true);
        for (channel, at) in [
            (Channel::Voltage, 2),
            (Channel::Ground, 6),
            (Channel::Temperature, 10),
        ] {
            let due: Vec<usize> = ticks
                .iter()
                .enumerate()
                .filter(|(_, (reading, _))| match reading {
                    Some(Reading::Voltage(_)) => channel == Channel::Voltage,
                    Some(Reading::Ground(_)) => channel == Channel::Ground,
                    Some(Reading::Temperature(_)) => channel == Channel::Temperature,
                    None => false,
                })
                .map(|(tick, _)| tick)
                .collect();
            let expected: Vec<usize> = (0..10).map(|n| at + 16 * n).collect();
            assert_eq!(due, expected, "{channel:?}");
        }
    }

    #[test]
    fn the_first_conversion_after_a_setup_is_never_a_reading() {
        let mut schedule = Schedule::new();
        let ticks = run(&mut schedule, 64, Some(1), true);
        for (tick, (reading, _)) in ticks.iter().enumerate().skip(1) {
            if ticks[tick - 1].1.is_some_and(|c| c.setup) {
                assert_eq!(*reading, None, "tick {tick}");
            }
        }
    }

    #[test]
    fn leaves_the_temperature_alone_when_not_asked() {
        let mut schedule = Schedule::new();
        for (reading, convert) in run(&mut schedule, 160, Some(1), false) {
            assert!(!matches!(reading, Some(Reading::Temperature(_))));
            assert!(convert.is_none_or(|c| c.channel != Channel::Temperature));
        }
    }

    #[test]
    fn decides_on_the_temperature_at_the_start_of_its_turn() {
        let mut schedule = Schedule::new();
        run(&mut schedule, 8, Some(1), false);
        assert_eq!(
            schedule.tick(Some(1), true),
            (None, convert(Channel::Temperature, true))
        );
        assert_eq!(
            schedule.tick(Some(1), false),
            (None, convert(Channel::Temperature, false))
        );
        assert_eq!(
            schedule.tick(Some(1), false),
            (Some(Reading::Temperature(1)), None)
        );
        run(&mut schedule, 13, Some(1), false);
        assert_eq!(schedule.tick(Some(1), false), (None, None));
        assert_eq!(schedule.tick(Some(1), true), (None, None));
    }

    #[test]
    fn sets_the_adc_up_at_the_start_of_every_turn() {
        let mut schedule = Schedule::new();
        let setups = |ticks: &[(Option<Reading>, Option<Convert>)]| -> Vec<Channel> {
            ticks
                .iter()
                .filter_map(|(_, convert)| convert.filter(|c| c.setup).map(|c| c.channel))
                .collect()
        };
        let off = run(&mut schedule, 64, Some(1), false);
        assert_eq!(setups(&off), [Channel::Voltage, Channel::Ground].repeat(4));
        let on = run(&mut schedule, 64, Some(1), true);
        let turns = [Channel::Voltage, Channel::Ground, Channel::Temperature];
        assert_eq!(setups(&on), turns.repeat(4));
    }

    #[test]
    fn a_rest_starts_the_round_again_and_sets_the_adc_up() {
        let mut schedule = Schedule::new();
        run(&mut schedule, 21, Some(1), true);
        schedule.rest();
        let ticks = run(&mut schedule, 3, Some(7), false);
        assert_eq!(ticks[0], (None, convert(Channel::Voltage, true)));
        assert_eq!(ticks[1], (None, convert(Channel::Voltage, false)));
        assert_eq!(ticks[2], (Some(Reading::Voltage(7)), None));
    }

    #[test]
    fn a_result_with_no_conversion_started_is_ignored() {
        let mut schedule = Schedule::new();
        let mut readings = Vec::new();
        for tick in 0..160 {
            let phase = tick % 16;
            let result = (![2, 6, 10].contains(&phase)).then_some(0x155);
            let (reading, _) = schedule.tick(result, true);
            readings.push((phase, reading));
        }
        for (phase, reading) in readings {
            match phase {
                2 => assert_eq!(reading, Some(Reading::Voltage(0))),
                6 => assert_eq!(reading, Some(Reading::Ground(u16::MAX))),
                10 => assert_eq!(reading, Some(Reading::Temperature(0))),
                _ => assert_eq!(reading, None, "phase {phase}"),
            }
        }
        let mut schedule = Schedule::new();
        for tick in 0..160 {
            let (reading, convert) = schedule.tick(Some(0x155), false);
            if tick % 16 >= 8 {
                assert_eq!((reading, convert), (None, None), "tick {tick}");
            }
        }
    }

    #[test]
    fn no_result_when_one_is_due_reads_as_zero_and_a_bad_ground() {
        let mut schedule = Schedule::new();
        let ticks = run(&mut schedule, 16, None, true);
        assert_eq!(ticks[2].0, Some(Reading::Voltage(0)));
        assert_eq!(ticks[6].0, Some(Reading::Ground(u16::MAX)));
        assert_eq!(ticks[10].0, Some(Reading::Temperature(0)));
    }
}
