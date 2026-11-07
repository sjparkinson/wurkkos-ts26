//! Test doubles shared by the library's own tests and xtask's simulators, so
//! there's one fake ADC, one way of turning a voltage or a temperature into a
//! raw reading, and one reader of docs/ui.md's scripts.
//!
//! The firmware never sees it.
//!
//! # Scripts
//!
//! A script is Anduril's notation, a word at a time: `1C 2H:1.5s wait:1s`.
//! `NC` is N quick clicks. `NH` is N - 1 clicks then a hold (so `2H` is
//! click, hold), for a second unless it says otherwise (`1H:500ms`).
//! `wait:2s` leaves the switch alone. Every click or hold is followed by
//! enough of a pause for it to count, so `1C 1C` is two single clicks.
//!
//! Some words set the scene, taking no time. `cell:2.95V` is the cell from
//! there on (3.8 V to start), and `heat:80C` the temperature (25 °C to
//! start). `fault` starts a torch whose power-on checks failed, so it has to
//! come before anything that takes time. So does `flash:corrupt`, a byte of
//! flash that doesn't match its CRC.

use crate::battery;
use crate::sensors::{Channel, Convert};
use crate::temp::Calibration;

/// The most four accumulated 10-bit conversions can read.
const FULL_SCALE: u16 = 4 * 1023;

/// A temperature sensor calibration for tests and the simulated chip's
/// signature row. Made up, but in the range real parts have: about 0.55 K a
/// count, so 25 °C reads 531 a conversion, 2124 over four.
pub const CALIBRATION: Calibration = Calibration {
    offset: -10,
    gain: 141,
};

/// The accumulated reading of AIN1 for a cell `battery::centivolts` reports
/// as `cell`: the closest, saturating at four full-scale conversions.
pub fn voltage_reading(cell: u16) -> u16 {
    closest(cell, battery::centivolts)
}

/// The temperature sensor reading, four accumulated 10-bit conversions,
/// closest to `kelvin` with [`CALIBRATION`].
pub fn temperature_reading(kelvin: u16) -> u16 {
    closest(kelvin.saturating_mul(16), |raw| CALIBRATION.sixteenths(raw))
}

/// The smallest raw reading whose `read` is closest to `target`. `read` never
/// goes down as the reading goes up, so a binary search finds it.
fn closest(target: u16, read: impl Fn(u16) -> u16) -> u16 {
    // The first reading that reads as at least `value`, or one past full
    // scale if none does.
    let first_at_least = |value: u16| {
        let (mut low, mut high) = (0, FULL_SCALE + 1);
        while low < high {
            let mid = low + (high - low) / 2;
            if read(mid) < value {
                low = mid + 1;
            } else {
                high = mid;
            }
        }
        low
    };
    let above = first_at_least(target);
    if above == 0 {
        return 0;
    }
    let below = read(above - 1);
    if above > FULL_SCALE || target - below <= read(above) - target {
        // Below wins ties, and a run of readings that read the same starts
        // further down.
        first_at_least(below)
    } else {
        above
    }
}

/// An ADC that hands back a conversion's result a tick after it started, the
/// way the firmware's loop sees the real one. A conversion measures what
/// was there when it started.
///
/// `voltage` and `temperature` are the raw results [`Adc::start`] reads.
/// Anything else that knows what each channel reads can use
/// [`Adc::start_with`] instead.
#[derive(Clone, Debug)]
pub struct Adc {
    pub voltage: u16,
    pub temperature: u16,
    pending: Option<u16>,
}

impl Default for Adc {
    fn default() -> Self {
        Self::new()
    }
}

impl Adc {
    /// 3.8 V and 25 °C with [`CALIBRATION`].
    pub const fn new() -> Self {
        Self {
            voltage: 3516,
            temperature: 2124,
            pending: None,
        }
    }

    /// The result of the conversion started last tick, if there was one.
    pub fn result(&mut self) -> Option<u16> {
        self.pending.take()
    }

    /// Starts `convert`, if there is one, reading `voltage` or
    /// `temperature`.
    pub fn start(&mut self, convert: Option<Convert>) {
        let (voltage, temperature) = (self.voltage, self.temperature);
        self.start_with(convert, |channel| match channel {
            Channel::Voltage => voltage,
            Channel::Temperature => temperature,
        });
    }

    /// Starts `convert`, if there is one, reading whatever `read` says the
    /// channel reads.
    pub fn start_with(&mut self, convert: Option<Convert>, read: impl FnOnce(Channel) -> u16) {
        self.pending = convert.map(|convert| read(convert.channel));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn convert(channel: Channel) -> Option<Convert> {
        Some(Convert {
            channel,
            setup: false,
        })
    }

    #[test]
    fn answers_for_the_channel_a_tick_later() {
        let mut adc = Adc::new();
        assert_eq!(adc.result(), None);
        adc.start(convert(Channel::Temperature));
        assert_eq!(adc.result(), Some(2124));
        assert_eq!(adc.result(), None);
        adc.start(convert(Channel::Voltage));
        assert_eq!(adc.result(), Some(3516));
        adc.start(None);
        assert_eq!(adc.result(), None);
    }

    #[test]
    fn measures_what_was_there_when_it_started() {
        let mut adc = Adc::new();
        adc.start(convert(Channel::Voltage));
        adc.voltage = 7;
        assert_eq!(adc.result(), Some(3516));
    }

    #[test]
    fn the_defaults_are_a_good_cell_at_room_temperature() {
        assert_eq!(battery::centivolts(Adc::new().voltage), 380);
        let room = crate::thermal::celsius(25);
        assert_eq!(temperature_reading(room), 2124);
        assert_eq!(CALIBRATION.sixteenths(2124), room * 16);
    }

    #[test]
    fn a_stuck_adc_is_garbage_to_thermal_regulation() {
        use crate::thermal::plausible;
        for raw in [0, 1, 7, 1023, FULL_SCALE] {
            assert!(!plausible(CALIBRATION.sixteenths(raw)), "{raw}");
        }
        assert!(plausible(CALIBRATION.sixteenths(Adc::new().temperature)));
    }

    #[test]
    fn readings_come_back_as_what_was_asked_for() {
        for cell in [250, 290, 295, 380, 420] {
            assert_eq!(battery::centivolts(voltage_reading(cell)), cell, "{cell}");
        }
        for kelvin in [273, 298, 323, 343, 373] {
            let got = CALIBRATION.sixteenths(temperature_reading(kelvin));
            // A count of the sum is a little over two sixteenths.
            assert!(got.abs_diff(kelvin * 16) <= 2, "{kelvin} K reads as {got}");
        }
    }

    /// The slow way: every raw reading, the closest winning and the smallest
    /// winning a tie.
    fn linear(target: u16, read: impl Fn(u16) -> u16) -> u16 {
        (0..=FULL_SCALE)
            .min_by_key(|&raw| (read(raw).abs_diff(target), raw))
            .unwrap_or(0)
    }

    #[test]
    fn the_binary_search_finds_what_the_linear_one_does() {
        let top = battery::centivolts(FULL_SCALE);
        for cell in 0..=top.saturating_add(10) {
            assert_eq!(
                voltage_reading(cell),
                linear(cell, battery::centivolts),
                "{cell} cV"
            );
        }
        let sixteenths = |raw| CALIBRATION.sixteenths(raw);
        for kelvin in 0..=sixteenths(FULL_SCALE) / 16 + 10 {
            assert_eq!(
                temperature_reading(kelvin),
                linear(kelvin * 16, sixteenths),
                "{kelvin} K"
            );
        }
    }
}
