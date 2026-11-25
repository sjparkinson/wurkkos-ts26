//! Scripts are Anduril's notation.
//!
//! `2H` is a click then a hold, for a second unless timed (`1H:500ms`), and a
//! hold that ends the script is never let go. `fault` and `flash:corrupt` only
//! go before anything that takes time.

mod examples;

#[cfg(test)]
pub use examples::example;
pub use examples::{EXAMPLES, Example, Expect};

use core::fmt;
use core::iter;
use core::ops::Deref;
use core::str::FromStr;

use crate::battery;
use crate::button::{Event, HOLD_TICKS, RELEASE_TICKS};
use crate::selftest::Faults;
use crate::sensors::{Channel, Convert};
use crate::temp::Calibration;
use crate::thermal;
use crate::torch::Torch;
use crate::watchdog::{Pace, REST_TICKS, Step};
use crate::{TICKS_PER_SECOND, u32_from};

const FULL_SCALE: u16 = 4 * 1023;

/// Made up, but in the range real parts have.
pub const CALIBRATION: Calibration = Calibration {
    offset: -10,
    gain: 141,
};

pub fn voltage_reading(cell: u16) -> u16 {
    closest(cell, battery::centivolts)
}

pub fn temperature_reading(kelvin: u16) -> u16 {
    closest(kelvin.saturating_mul(16), |raw| CALIBRATION.sixteenths(raw))
}

fn closest(target: u16, read: impl Fn(u16) -> u16) -> u16 {
    let first_at_least = |value: u16| {
        let (mut low, mut high) = (0, FULL_SCALE + 1);
        while low < high {
            let mid = low.midpoint(high);
            if read(mid) < value {
                low = mid.saturating_add(1);
            } else {
                high = mid;
            }
        }
        low
    };
    let above = first_at_least(target);
    let Some(under) = above.checked_sub(1) else {
        return 0;
    };
    let below = read(under);
    if above > FULL_SCALE || target.abs_diff(below) <= read(above).abs_diff(target) {
        first_at_least(below)
    } else {
        above
    }
}

#[derive(Clone, Debug)]
pub struct Adc {
    pub voltage: u16,
    pub ground: u16,
    pub temperature: u16,
    pending: Option<u16>,
}

impl Default for Adc {
    fn default() -> Self {
        Self::new()
    }
}

impl Adc {
    pub const fn new() -> Self {
        Self {
            voltage: 3516,
            ground: 0,
            temperature: 2124,
            pending: None,
        }
    }

    pub const fn result(&mut self) -> Option<u16> {
        self.pending.take()
    }

    pub fn start(&mut self, convert: Option<Convert>) {
        let (voltage, ground, temperature) = (self.voltage, self.ground, self.temperature);
        self.start_with(convert, |channel| match channel {
            Channel::Voltage => voltage,
            Channel::Ground => ground,
            Channel::Temperature => temperature,
        });
    }

    pub fn start_with(&mut self, convert: Option<Convert>, read: impl FnOnce(Channel) -> u16) {
        self.pending = convert.map(|convert| read(convert.channel));
    }
}

const PRESS_TICKS: u16 = 4;
const GAP_TICKS: u16 = 4;
const SETTLE_TICKS: u16 = RELEASE_TICKS + 1;
const DEFAULT_HOLD_TICKS: u16 = TICKS_PER_SECOND;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Clicks(u8),
    Hold { clicks: u8, ticks: u16 },
    Wait(u16),
    Cell(u16),
    Heat(u16),
    CorruptFlash,
    Fault,
}

impl Action {
    const fn takes_time(self) -> bool {
        matches!(self, Self::Clicks(_) | Self::Hold { .. } | Self::Wait(_))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Count,
    Kind,
    TimedClick,
    ShortHold,
    Duration,
    UnderATick,
    TooLong,
    Voltage,
    Temperature,
    NotAtPowerOn,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = match self {
            Self::Count => "expected a count of 1 or more, like 1C, 2H or wait:1s",
            Self::Kind => "expected C (clicks), H (hold) or wait:",
            Self::TimedClick => "clicks don't take a duration; holds do (1H:2s)",
            Self::ShortHold => {
                const MS: u32 = u32_from(HOLD_TICKS) * 1000 / u32_from(TICKS_PER_SECOND);
                return write!(f, "that's a click; a hold needs longer than {MS} ms");
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
                    clicks: count.checked_sub(1).ok_or(Error::Count)?,
                    ticks,
                })
            }
            _ => Err(Error::Kind),
        }
    }
}

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

fn decimal(text: &str, places: u32) -> Option<u32> {
    let (whole, fraction) = text.split_once('.').unwrap_or((text, ""));
    let fits = u32::try_from(fraction.len()).is_ok_and(|digits| digits <= places);
    if !fits || !fraction.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let mut unit = 10u32.pow(places);
    let mut value = whole.parse::<u32>().ok()?.checked_mul(unit)?;
    for digit in fraction.bytes() {
        unit /= 10;
        let digit = char::from(digit).to_digit(10)?;
        value = value.checked_add(digit.checked_mul(unit)?)?;
    }
    Some(value)
}

fn centivolts(volts: &str) -> Result<u16, Error> {
    volts
        .strip_suffix('V')
        .and_then(|number| decimal(number, 2))
        .and_then(|cv| u16::try_from(cv).ok())
        .ok_or(Error::Voltage)
}

fn ticks(duration: &str) -> Result<u16, Error> {
    let ms = if let Some(ms) = duration.strip_suffix("ms") {
        ms.parse().ok()
    } else if let Some(s) = duration.strip_suffix('s') {
        decimal(s, 3)
    } else {
        None
    };
    let ms = ms.ok_or(Error::Duration)?;
    let rounded = u64::from(ms)
        .checked_mul(u64::from(TICKS_PER_SECOND))
        .and_then(|scaled| scaled.checked_add(500))
        .ok_or(Error::TooLong)?;
    match u16::try_from(rounded / 1000) {
        Ok(0) => Err(Error::UnderATick),
        Ok(ticks) => Ok(ticks),
        Err(_) => Err(Error::TooLong),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AdcFault {
    Dead,
    Frozen,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Scene {
    pub cell: u16,
    pub kelvin: u16,
    pub fault: bool,
    pub corrupt_flash: bool,
    pub adc: Option<AdcFault>,
}

impl Scene {
    pub const NORMAL: Self = Self {
        cell: 380,
        kelvin: thermal::celsius(25),
        fault: false,
        corrupt_flash: false,
        adc: None,
    };

    pub const fn faults(&self) -> Faults {
        Faults {
            fuses: self.fault,
            flash: self.corrupt_flash,
            ..Faults::NONE
        }
    }

    pub fn reading(&self, channel: Channel) -> u16 {
        match channel {
            _ if self.adc == Some(AdcFault::Frozen) => voltage_reading(self.cell),
            Channel::Voltage => voltage_reading(self.cell),
            Channel::Ground => 0,
            Channel::Temperature => temperature_reading(self.kelvin),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sample {
    pub pressed: bool,
    pub scene: Scene,
}

pub fn samples(actions: impl IntoIterator<Item = Action>) -> impl Iterator<Item = Sample> {
    let mut actions = actions.into_iter().peekable();
    let words = iter::from_fn(move || {
        let action = actions.next()?;
        Some((action, actions.peek().is_none()))
    });
    let mut scene = Scene::NORMAL;
    words.flat_map(move |(action, last)| {
        let (quick, held, released) = match action {
            Action::Clicks(n) => (n, 0, SETTLE_TICKS),
            Action::Hold { clicks, ticks } => (clicks, ticks, if last { 0 } else { SETTLE_TICKS }),
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

pub fn clicks(n: u8) -> impl Iterator<Item = bool> {
    (0..n).flat_map(|_| span(true, PRESS_TICKS).chain(span(false, GAP_TICKS)))
}

fn span(pressed: bool, ticks: u16) -> impl Iterator<Item = bool> {
    iter::repeat_n(pressed, ticks.into())
}

#[derive(Clone, Debug)]
pub struct Rig {
    pub torch: Torch,
    pub adc: Adc,
    pub pace: Pace,
    frame: u16,
    pressed: bool,
    pub ticked: bool,
}

impl Default for Rig {
    fn default() -> Self {
        Self::new(Faults::NONE)
    }
}

impl Rig {
    pub const fn new(faults: Faults) -> Self {
        Self {
            torch: Torch::new(CALIBRATION, faults),
            adc: Adc::new(),
            pace: Pace::new(),
            frame: 0,
            pressed: false,
            ticked: false,
        }
    }

    pub const fn powers_down(&self) -> bool {
        self.torch.resting() && self.pace.powers_down()
    }

    pub fn tick(&mut self, pressed: bool) -> Option<Event> {
        let event = self.torch.tick(pressed, self.adc.result(), 1);
        self.adc.start(self.torch.convert());
        event
    }

    pub fn tick_in(&mut self, pressed: bool, scene: &Scene) -> Option<Event> {
        self.frame = self.frame.wrapping_add(1) % REST_TICKS;
        let changed = core::mem::replace(&mut self.pressed, pressed) != pressed;
        let slow = self.pace.slow();
        if slow && changed {
            assert_eq!(self.pace.woken(), Some(Step::Fast));
        }
        self.ticked = !slow || (self.frame == 0 && !changed);
        if !self.ticked {
            return None;
        }
        let event = self
            .torch
            .tick(pressed, self.adc.result(), self.pace.ticks());
        let convert = self
            .torch
            .convert()
            .filter(|_| scene.adc != Some(AdcFault::Dead));
        self.adc
            .start_with(convert, |channel| scene.reading(channel));
        self.pace.tick(self.torch.slow());
        event
    }

    pub fn play(actions: impl IntoIterator<Item = Action>) -> Self {
        Self::play_with(actions, |_, _| {})
    }

    pub fn play_with(
        actions: impl IntoIterator<Item = Action>,
        mut each: impl FnMut(&Self, Option<Event>),
    ) -> Self {
        let mut samples = samples(actions).peekable();
        let power_on = samples.peek().map_or(Scene::NORMAL, |s| s.scene);
        let mut rig = Self::new(power_on.faults());
        for Sample { pressed, scene } in samples {
            let event = rig.tick_in(pressed, &scene);
            each(&rig, event);
        }
        rig
    }
}

impl Deref for Rig {
    type Target = Torch;

    fn deref(&self) -> &Torch {
        &self.torch
    }
}

#[derive(Clone, Debug)]
pub struct Rng(u32);

impl Rng {
    /// Xorshift never leaves zero.
    pub const fn new(seed: u32) -> Self {
        Self(if seed == 0 { 1 } else { seed })
    }

    pub const fn next_u32(&mut self) -> u32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        self.0
    }

    pub const fn next_u16(&mut self) -> u16 {
        let [_, _, low, high] = self.next_u32().to_le_bytes();
        u16::from_le_bytes([low, high])
    }
}

#[cfg(test)]
mod tests {
    extern crate std;

    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn the_rng_doesnt_get_stuck_at_zero() {
        for seed in [0, 1, 0xDEAD_BEEF] {
            let mut rng = Rng::new(seed);
            let values: BTreeSet<u16> = (0..1000).map(|_| rng.next_u16()).collect();
            assert!(values.len() > 900, "seed {seed}: {} values", values.len());
            assert!(values.iter().any(|&value| value > 0xF000));
        }
    }

    const fn convert(channel: Channel) -> Convert {
        Convert {
            channel,
            setup: false,
        }
    }

    #[test]
    fn answers_for_the_channel_a_tick_later() {
        let mut adc = Adc::new();
        assert_eq!(adc.result(), None);
        adc.start(Some(convert(Channel::Temperature)));
        assert_eq!(adc.result(), Some(2124));
        assert_eq!(adc.result(), None);
        adc.start(Some(convert(Channel::Voltage)));
        assert_eq!(adc.result(), Some(3516));
        adc.start(None);
        assert_eq!(adc.result(), None);
    }

    #[test]
    fn measures_what_was_there_when_it_started() {
        let mut adc = Adc::new();
        adc.start(Some(convert(Channel::Voltage)));
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
            assert!(got.abs_diff(kelvin * 16) <= 2, "{kelvin} K reads as {got}");
        }
    }

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
    fn a_script_that_ends_in_a_hold_ends_held() {
        let played = |script: &'static [&'static str]| {
            samples(parse(script.iter().copied()).map(Result::unwrap))
        };
        let held = |script| played(script).last().map(|sample| sample.pressed);
        assert_eq!(held(&["1H"]), Some(true));
        assert_eq!(held(&["2C", "1H:2s"]), Some(true));
        assert_eq!(held(&["1H", "1C"]), Some(false));
        assert_eq!(held(&["1H", "wait:1s"]), Some(false));
        let ticks = |script| played(script).count();
        assert_eq!(ticks(&["1H"]), usize::from(DEFAULT_HOLD_TICKS));
        assert_eq!(
            ticks(&["1H", "1H"]),
            usize::from(2 * DEFAULT_HOLD_TICKS + SETTLE_TICKS)
        );
    }

    #[test]
    fn a_change_of_scene_takes_no_time_and_lasts() {
        let script = parse(["wait:1s", "cell:2.95V", "wait:1s"]).map(Result::unwrap);
        let cells = samples(script).map(|sample| sample.scene.cell);
        assert!(cells.eq(iter::repeat_n(380, 64).chain(iter::repeat_n(295, 64))));
    }

    #[test]
    fn a_cell_keeps_its_decimal_point() {
        assert_eq!(word("cell:3.8V"), Ok(Action::Cell(380)));
        assert_eq!(word("cell:3.05V"), Ok(Action::Cell(305)));
        assert_eq!(word("cell:4V"), Ok(Action::Cell(400)));
    }

    #[test]
    fn a_cell_outside_the_plausible_range_reads_as_one_the_firmware_cant_believe() {
        assert_eq!(word("cell:0V"), Ok(Action::Cell(0)));
        assert_eq!(voltage_reading(0), 0);
        let reads = |cell: &'static str| {
            let script = parse([cell, "wait:1s"]).map(Result::unwrap);
            let scene = samples(script).last().unwrap().scene;
            battery::centivolts(scene.reading(Channel::Voltage))
        };
        for cell in ["cell:0V", "cell:0.99V", "cell:4.41V", "cell:5V"] {
            assert!(!battery::plausible(reads(cell)), "{cell}");
        }
        for cell in ["cell:1V", "cell:3.8V", "cell:4.4V"] {
            assert!(battery::plausible(reads(cell)), "{cell}");
        }
    }

    #[test]
    fn a_fault_keeps_the_cell_and_heat_before_it() {
        let script = parse(["cell:2.95V", "heat:80C", "fault", "1C"]).map(Result::unwrap);
        let expected = Scene {
            cell: 295,
            kelvin: thermal::celsius(80),
            fault: true,
            corrupt_flash: false,
            adc: None,
        };
        let mut samples = samples(script).peekable();
        assert!(samples.peek().is_some());
        assert!(samples.all(|sample| sample.scene == expected));
        assert!(expected.faults().fuses);
    }

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

    #[test]
    fn a_rig_starts_off_and_plays_a_script() {
        assert_eq!(Rig::default().level(), 0);
        let script = parse(["1C"]).map(Result::unwrap);
        assert_eq!(Rig::play(script).level(), crate::ui::ON_LEVEL);
        let mut ticks = 0;
        let mut clicks = 0;
        Rig::play_with(parse(["2C", "wait:1s"]).map(Result::unwrap), |_, event| {
            ticks += 1;
            clicks += usize::from(event == Some(Event::Click(2)));
        });
        assert_eq!(clicks, 1);
        assert_eq!(ticks, 2 * 8 + SETTLE_TICKS + TICKS_PER_SECOND);
    }

    #[test]
    fn a_scripts_fault_is_a_power_on_fault() {
        let rig = Rig::play(parse(["fault", "1C"]).map(Result::unwrap));
        assert_eq!(rig.level(), 0);
        assert_eq!(rig.torch.aux(), crate::ui::Aux::White);
    }
}
