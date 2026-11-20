//! The one ADC, shared between the cell's voltage and the temperature.
//!
//! The first conversion after a setup is thrown away. The datasheet doesn't
//! ask for that, but gives the ADC settling time on a new channel and the
//! reference about 25 µs to start (DS40002204A §30.3.2.4, §36.15), and PA1
//! sits behind tens of kilohms of divider.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Channel {
    Voltage,
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
    Temperature(u16),
}

/// Four readings a second of each channel, which `battery::Protection` and
/// `thermal::Thermal` assume.
pub const CYCLE_TICKS: u8 = 16;
const TEMPERATURE_AT: u8 = CYCLE_TICKS / 2;

const _: () = assert!(CYCLE_TICKS.is_power_of_two());

#[derive(Clone, Debug, Default)]
pub struct Schedule {
    phase: u8,
    temperature: bool,
    /// Back to `None` on [`Schedule::rest`], since the firmware turns the
    /// reference off to power down.
    set_up: Option<Channel>,
}

impl Schedule {
    pub const fn new() -> Self {
        Self {
            phase: 0,
            temperature: false,
            set_up: None,
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
        let (channel, step) = match phase.checked_sub(TEMPERATURE_AT) {
            None => (Channel::Voltage, phase),
            Some(step) => (Channel::Temperature, step),
        };
        if step == 0 && channel == Channel::Temperature {
            self.temperature = temperature;
        }
        if channel == Channel::Temperature && !self.temperature {
            return (None, None);
        }
        let convert = |setup| Some(Convert { channel, setup });
        match step {
            0 => {
                let setup = self.set_up != Some(channel);
                self.set_up = Some(channel);
                (None, convert(setup))
            }
            1 => (None, convert(false)),
            2 => {
                let reading = result.unwrap_or(0);
                let reading = match channel {
                    Channel::Voltage => Reading::Voltage(reading),
                    Channel::Temperature => Reading::Temperature(reading),
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
    fn reads_the_voltage_then_the_temperature_8_ticks_later() {
        let mut schedule = Schedule::new();
        let ticks = run(&mut schedule, 16, Some(7), true);
        let mut expected = [(None, None); 16];
        expected[0].1 = convert(Channel::Voltage, true);
        expected[1].1 = convert(Channel::Voltage, false);
        expected[2].0 = Some(Reading::Voltage(7));
        expected[8].1 = convert(Channel::Temperature, true);
        expected[9].1 = convert(Channel::Temperature, false);
        expected[10].0 = Some(Reading::Temperature(7));
        assert_eq!(ticks, expected);
    }

    #[test]
    fn each_channel_comes_round_every_16_ticks() {
        let mut schedule = Schedule::new();
        let ticks = run(&mut schedule, 160, Some(1), true);
        for (channel, at) in [(Channel::Voltage, 2), (Channel::Temperature, 10)] {
            let due: Vec<usize> = ticks
                .iter()
                .enumerate()
                .filter(|(_, (reading, _))| match reading {
                    Some(Reading::Voltage(_)) => channel == Channel::Voltage,
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
            assert!(convert.is_none_or(|c| c.channel == Channel::Voltage));
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
    fn only_sets_the_adc_up_when_the_channel_changes() {
        let mut schedule = Schedule::new();
        let setups = |ticks: &[(Option<Reading>, Option<Convert>)]| -> Vec<Channel> {
            ticks
                .iter()
                .filter_map(|(_, convert)| convert.filter(|c| c.setup).map(|c| c.channel))
                .collect()
        };
        let off = run(&mut schedule, 64, Some(1), false);
        assert_eq!(off[0].1, convert(Channel::Voltage, true));
        assert_eq!(off[16].1, convert(Channel::Voltage, false));
        assert_eq!(setups(&off), [Channel::Voltage]);
        let on = run(&mut schedule, 64, Some(1), true);
        let alternating = [Channel::Temperature, Channel::Voltage].repeat(4);
        assert_eq!(setups(&on)[..], alternating[..7]);
        let off = run(&mut schedule, 64, Some(1), false);
        assert_eq!(setups(&off), [Channel::Voltage]);
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
            let result = (phase != 2 && phase != 10).then_some(0x155);
            let (reading, _) = schedule.tick(result, true);
            readings.push((phase, reading));
        }
        for (phase, reading) in readings {
            match phase {
                2 => assert_eq!(reading, Some(Reading::Voltage(0))),
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
    fn no_result_when_one_is_due_reads_as_zero() {
        let mut schedule = Schedule::new();
        let ticks = run(&mut schedule, 16, None, true);
        assert_eq!(ticks[2].0, Some(Reading::Voltage(0)));
        assert_eq!(ticks[10].0, Some(Reading::Temperature(0)));
    }
}
