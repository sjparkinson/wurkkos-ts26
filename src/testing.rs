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

use core::fmt;
use core::iter;
use core::str::FromStr;

use crate::TICKS_PER_SECOND;
use crate::battery;
use crate::button::{HOLD_TICKS, RELEASE_TICKS};
use crate::selftest::Faults;
use crate::sensors::{Channel, Convert};
use crate::temp::Calibration;
use crate::thermal;

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

/// How long a scripted click holds the switch down (about 60 ms).
const PRESS_TICKS: u16 = 4;
/// The pause after each scripted click, well inside `RELEASE_TICKS`.
const GAP_TICKS: u16 = 4;
/// Released samples after a click or hold before the next action: enough
/// for `button::Clicks` to send the click (see its `RELEASE_TICKS` check).
const SETTLE_TICKS: u16 = RELEASE_TICKS + 1;
/// A hold with no duration lasts a second.
const DEFAULT_HOLD_TICKS: u16 = TICKS_PER_SECOND;

/// One word of a script.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// This many quick clicks.
    Clicks(u8),
    /// `clicks` quick clicks, then the switch held down for `ticks`.
    Hold { clicks: u8, ticks: u16 },
    /// The switch left alone for this many ticks.
    Wait(u16),
    /// The cell from now on, in centivolts.
    Cell(u16),
    /// The temperature from now on, in kelvin.
    Heat(u16),
    /// A flash byte that doesn't match the CRC. Power-on only.
    CorruptFlash,
    /// The power-on checks fail. Power-on only.
    Fault,
}

impl Action {
    /// Whether it takes time, rather than setting the scene.
    fn takes_time(self) -> bool {
        matches!(self, Self::Clicks(_) | Self::Hold { .. } | Self::Wait(_))
    }
}

/// Why a word isn't an [`Action`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// No count of 1 or more before the C or H.
    Count,
    /// Not clicks, a hold or anything else a script knows.
    Kind,
    /// A click with a duration.
    TimedClick,
    /// A hold too short to count as one.
    ShortHold,
    /// A duration it can't read.
    Duration,
    /// A duration that rounds to no ticks.
    UnderATick,
    /// A duration of more ticks than a `u16` holds.
    TooLong,
    Voltage,
    Temperature,
    /// `fault` or `flash:corrupt` after something that takes time.
    NotAtPowerOn,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = match self {
            Self::Count => "expected a count of 1 or more, like 1C, 2H or wait:1s",
            Self::Kind => "expected C (clicks), H (hold) or wait:",
            Self::TimedClick => "clicks don't take a duration; holds do (1H:2s)",
            Self::ShortHold => {
                let ms = u32::from(HOLD_TICKS) * 1000 / u32::from(TICKS_PER_SECOND);
                return write!(f, "that's a click; a hold needs longer than {ms} ms");
            }
            Self::Duration => "expected a duration like 500ms, 2s or 1.5s",
            Self::UnderATick => "that's shorter than a tick",
            Self::TooLong => "that's too long",
            Self::Voltage => "expected a voltage like 2.95V",
            Self::Temperature => "expected a temperature like 80C",
            Self::NotAtPowerOn => {
                "the firmware only checks that at power-on, so it goes before anything \
                 that takes time"
            }
        };
        f.write_str(text)
    }
}

impl FromStr for Action {
    type Err = Error;

    /// `3C`, `2H`, `1H:500ms`, `wait:2s`, `cell:2.95V`, `heat:80C`, `fault`
    /// or `flash:corrupt`.
    fn from_str(word: &str) -> Result<Self, Error> {
        if word == "fault" {
            return Ok(Self::Fault);
        }
        if word == "flash:corrupt" {
            return Ok(Self::CorruptFlash);
        }
        if let Some(volts) = word.strip_prefix("cell:") {
            return centivolts(volts).map(Self::Cell);
        }
        if let Some(celsius) = word.strip_prefix("heat:") {
            let degrees: u8 = celsius
                .strip_suffix('C')
                .and_then(|degrees| degrees.parse().ok())
                .ok_or(Error::Temperature)?;
            return Ok(Self::Heat(thermal::celsius(degrees)));
        }
        if let Some(duration) = word.strip_prefix("wait:") {
            return ticks(duration).map(Self::Wait);
        }
        let (head, duration) = match word.split_once(':') {
            Some((head, duration)) => (head, Some(duration)),
            None => (word, None),
        };
        let (count, kind) = head.split_at(head.len().saturating_sub(1));
        let count: u8 = match count.parse() {
            Ok(n) if n > 0 => n,
            _ => return Err(Error::Count),
        };
        match (kind, duration) {
            ("C" | "c", None) => Ok(Self::Clicks(count)),
            ("C" | "c", Some(_)) => Err(Error::TimedClick),
            ("H" | "h", duration) => {
                let ticks = duration.map_or(Ok(DEFAULT_HOLD_TICKS), ticks)?;
                if ticks <= HOLD_TICKS {
                    return Err(Error::ShortHold);
                }
                Ok(Self::Hold {
                    clicks: count - 1,
                    ticks,
                })
            }
            _ => Err(Error::Kind),
        }
    }
}

/// A word of a script and why it isn't one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScriptError<'a> {
    pub word: &'a str,
    pub error: Error,
}

impl fmt::Display for ScriptError<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "`{}`: {}", self.word, self.error)
    }
}

/// Reads a script, word by word.
pub fn parse<'a>(
    words: impl IntoIterator<Item = &'a str>,
) -> impl Iterator<Item = Result<Action, ScriptError<'a>>> {
    let mut started = false;
    words.into_iter().map(move |word| {
        let at = |error| ScriptError { word, error };
        let action: Action = word.parse().map_err(at)?;
        if matches!(action, Action::Fault | Action::CorruptFlash) && started {
            return Err(at(Error::NotAtPowerOn));
        }
        started |= action.takes_time();
        Ok(action)
    })
}

/// `whole.fraction`, times ten to the `places`, with no more than `places`
/// digits after the point.
fn decimal(text: &str, places: u32) -> Option<u32> {
    let (whole, fraction) = text.split_once('.').unwrap_or((text, ""));
    if fraction.len() > places as usize || !fraction.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let mut unit = 10u32.pow(places);
    let mut value = whole.parse::<u32>().ok()?.checked_mul(unit)?;
    for digit in fraction.bytes() {
        unit /= 10;
        value = value.checked_add(u32::from(digit - b'0') * unit)?;
    }
    Some(value)
}

/// Centivolts from `2.95V`.
fn centivolts(volts: &str) -> Result<u16, Error> {
    volts
        .strip_suffix('V')
        .and_then(|number| decimal(number, 2))
        .and_then(|cv| u16::try_from(cv).ok())
        .ok_or(Error::Voltage)
}

/// Ticks from `500ms`, `2s` or `1.5s`, rounded to the nearest tick.
fn ticks(duration: &str) -> Result<u16, Error> {
    let ms = if let Some(ms) = duration.strip_suffix("ms") {
        ms.parse().ok()
    } else if let Some(s) = duration.strip_suffix('s') {
        decimal(s, 3)
    } else {
        None
    };
    let ms = ms.ok_or(Error::Duration)?;
    let ticks = (u64::from(ms) * u64::from(TICKS_PER_SECOND) + 500) / 1000;
    match u16::try_from(ticks) {
        Ok(0) => Err(Error::UnderATick),
        Ok(ticks) => Ok(ticks),
        Err(_) => Err(Error::TooLong),
    }
}

/// What the torch sees, as far as a script says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Scene {
    /// The cell, in centivolts, as `battery::centivolts` would report it.
    pub cell: u16,
    pub kelvin: u16,
    /// The power-on checks fail.
    pub fault: bool,
    /// A flash byte doesn't match its CRC.
    pub corrupt_flash: bool,
}

impl Scene {
    /// A cell at 3.8 V, a room at 25 °C and nothing wrong.
    pub const NORMAL: Self = Self {
        cell: 380,
        kelvin: thermal::celsius(25),
        fault: false,
        corrupt_flash: false,
    };

    /// What the power-on checks find, with `fault` standing for a fuse that
    /// fails its read-back.
    pub fn faults(&self) -> Faults {
        Faults {
            fuses: self.fault,
            flash: self.corrupt_flash,
            ..Faults::NONE
        }
    }

    /// The raw reading a conversion on `channel` gives.
    pub fn reading(&self, channel: Channel) -> u16 {
        match channel {
            Channel::Voltage => voltage_reading(self.cell),
            Channel::Temperature => temperature_reading(self.kelvin),
        }
    }
}

/// One tick of a script: the switch, and what the torch sees.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sample {
    pub pressed: bool,
    pub scene: Scene,
}

/// The switch and the scene, one sample per tick, for a whole script.
pub fn samples(actions: impl IntoIterator<Item = Action>) -> impl Iterator<Item = Sample> {
    let mut scene = Scene::NORMAL;
    actions.into_iter().flat_map(move |action| {
        let (quick, held, released) = match action {
            Action::Clicks(n) => (n, 0, SETTLE_TICKS),
            Action::Hold { clicks, ticks } => (clicks, ticks, SETTLE_TICKS),
            Action::Wait(ticks) => (0, 0, ticks),
            Action::Cell(cell) => {
                scene.cell = cell;
                (0, 0, 0)
            }
            Action::Heat(kelvin) => {
                scene.kelvin = kelvin;
                (0, 0, 0)
            }
            Action::CorruptFlash => {
                scene.corrupt_flash = true;
                (0, 0, 0)
            }
            Action::Fault => {
                scene.fault = true;
                (0, 0, 0)
            }
        };
        let scene = scene;
        clicks(quick)
            .chain(span(true, held))
            .chain(span(false, released))
            .map(move |pressed| Sample { pressed, scene })
    })
}

/// `n` quick clicks, each a press then a short gap, one sample per tick.
pub fn clicks(n: u8) -> impl Iterator<Item = bool> {
    (0..n).flat_map(|_| span(true, PRESS_TICKS).chain(span(false, GAP_TICKS)))
}

fn span(pressed: bool, ticks: u16) -> impl Iterator<Item = bool> {
    iter::repeat_n(pressed, ticks.into())
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

    fn word(word: &str) -> Result<Action, Error> {
        word.parse()
    }

    #[test]
    fn parses_clicks_holds_and_waits() {
        let script = "1C 3c 1H 2H:500ms wait:1.5s wait:2s".split_whitespace();
        let expected = [
            Action::Clicks(1),
            Action::Clicks(3),
            Action::Hold {
                clicks: 0,
                ticks: 64,
            },
            Action::Hold {
                clicks: 1,
                ticks: 32,
            },
            Action::Wait(96),
            Action::Wait(128),
        ];
        assert!(parse(script).eq(expected.map(Ok)));
    }

    #[test]
    fn parses_the_words_that_set_the_scene() {
        let script = ["cell:2.95V", "heat:80C", "fault", "flash:corrupt"];
        let expected = [
            Action::Cell(295),
            Action::Heat(353),
            Action::Fault,
            Action::CorruptFlash,
        ];
        assert!(parse(script).eq(expected.map(Ok)));
        for bad in ["cell:3", "cell:2.955V", "heat:80", "heat:hotC", "heat:999C"] {
            assert!(word(bad).is_err(), "{bad:?} parsed");
        }
        assert!(word("flash:fine").is_err());
    }

    #[test]
    fn a_fault_only_goes_before_anything_that_takes_time() {
        assert!(parse(["cell:3.1V", "fault", "flash:corrupt", "1C"]).all(|a| a.is_ok()));
        for late in ["fault", "flash:corrupt"] {
            let error = ScriptError {
                word: late,
                error: Error::NotAtPowerOn,
            };
            assert_eq!(parse(["wait:1s", late]).find_map(Result::err), Some(error));
        }
    }

    #[test]
    fn rejects_what_it_cant_read() {
        for bad in [
            "0C",
            "C",
            "1X",
            "",
            "1C:1s",
            "1H:fast",
            "1H:1.2345s",
            "1H:100ms",
            "wait:0ms",
            "wait:2",
            "wait:1.-5s",
            "wait:9999s",
            "256C",
        ] {
            assert!(word(bad).is_err(), "{bad:?} parsed");
        }
    }

    #[test]
    fn rounds_durations_to_the_nearest_tick() {
        assert_eq!(ticks("8ms"), Ok(1));
        assert_eq!(ticks("15ms"), Ok(1));
        assert_eq!(ticks("24ms"), Ok(2));
        assert_eq!(ticks("1s"), Ok(64));
        assert_eq!(ticks("0.25s"), Ok(16));
    }

    #[test]
    fn a_change_of_scene_takes_no_time_and_lasts() {
        let script = parse(["wait:1s", "cell:2.95V", "wait:1s"]).map(Result::unwrap);
        let cells = samples(script).map(|sample| sample.scene.cell);
        assert!(cells.eq(iter::repeat_n(380, 64).chain(iter::repeat_n(295, 64))));
    }

    /// The torch tests used to read `cell:3.8V` as 0.38 V, dropping the dot.
    #[test]
    fn a_cell_keeps_its_decimal_point() {
        assert_eq!(word("cell:3.8V"), Ok(Action::Cell(380)));
        assert_eq!(word("cell:3.05V"), Ok(Action::Cell(305)));
        assert_eq!(word("cell:4V"), Ok(Action::Cell(400)));
    }

    /// The torch tests used to start a fresh bench for `fault`, forgetting the
    /// cell and the heat set before it.
    #[test]
    fn a_fault_keeps_the_cell_and_heat_before_it() {
        let script = parse(["cell:2.95V", "heat:80C", "fault", "1C"]).map(Result::unwrap);
        let expected = Scene {
            cell: 295,
            kelvin: thermal::celsius(80),
            fault: true,
            corrupt_flash: false,
        };
        let mut samples = samples(script).peekable();
        assert!(samples.peek().is_some());
        assert!(samples.all(|sample| sample.scene == expected));
        assert!(expected.faults().fuses);
    }

    /// The torch tests used to work out `wait:1500ms` as 1500 × 64 in a `u16`,
    /// which overflows, and couldn't read `1.5s` or a lower-case `c` or `h`.
    #[test]
    fn durations_and_lower_case_read_as_simulate_reads_them() {
        assert_eq!(word("wait:1500ms"), Ok(Action::Wait(96)));
        assert_eq!(word("wait:1.5s"), Ok(Action::Wait(96)));
        assert_eq!(word("2c"), Ok(Action::Clicks(2)));
        assert_eq!(
            word("2h:1.5s"),
            Ok(Action::Hold {
                clicks: 1,
                ticks: 96
            })
        );
    }
}
