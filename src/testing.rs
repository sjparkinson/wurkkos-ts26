//! Test doubles shared by the library's own tests and xtask's simulators, so
//! there's one fake ADC, one torch wired to it, one way of turning a voltage
//! or a temperature into a raw reading, and one reader of docs/ui.md.
//!
//! The firmware never sees it.
//!
//! # Scripts
//!
//! A script is Anduril's notation, a word at a time, like `1C 2H:1.5s wait:1s`.
//! `NC` is N quick clicks. `NH` is N - 1 clicks then a hold (so `2H` is
//! click, hold), for a second unless it says otherwise (`1H:500ms`).
//! `wait:2s` leaves the switch alone. Every click or hold is followed by
//! enough of a pause for it to count, so `1C 1C` is two single clicks,
//! except a hold that's the script's last word. That one's never let go, so
//! the script ends with the switch still down.
//!
//! Some words set the scene, taking no time. `cell:2.95V` is the cell from
//! there on (3.8 V to start), and `heat:80C` the temperature (25 °C to
//! start). A cell outside `battery::PLAUSIBLE`, such as `cell:0V`, is one
//! the firmware can't read, and 0 V is what a dead ADC reads. `fault` starts
//! a torch whose power-on checks failed, so it has to come before anything
//! that takes time. So does `flash:corrupt`, a byte of flash that doesn't
//! match its CRC.
//!
//! # docs/ui.md
//!
//! Every table with an Action column needs an Example column, and every row
//! in it an example unless its Status is Planned. Each is a script, then how
//! the light should be on its last tick, starting from a torch that's just had
//! its battery put in.
//!
//! ```text
//! | Action | Does | Example         | Status |
//! |--------|------|-----------------|--------|
//! | 1C     | On   | `1C` → level 50 | Done   |
//! ```
//!
//! [`examples`] reads them. `cargo test` plays each on [`Rig`], and
//! `cargo xtask hex` plays each on the firmware too.

use core::fmt;
use core::iter;
use core::ops::Deref;
use core::str::{FromStr, Lines};

use crate::TICKS_PER_SECOND;
use crate::battery;
use crate::button::{Event, HOLD_TICKS, RELEASE_TICKS};
use crate::selftest::Faults;
use crate::sensors::{Channel, Convert};
use crate::temp::Calibration;
use crate::thermal;
use crate::torch::Torch;

/// The most four accumulated 10-bit conversions can read.
const FULL_SCALE: u16 = 4 * 1023;

/// A temperature sensor calibration for tests and the simulated chip's
/// signature row. Made up, but in the range real parts have, at about 0.55 K
/// a count, so 25 °C reads 531 a conversion, 2124 over four.
pub const CALIBRATION: Calibration = Calibration {
    offset: -10,
    gain: 141,
};

/// The accumulated reading of AIN1 for a cell `battery::centivolts` reports
/// as `cell`, the closest, saturating at four full-scale conversions.
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
/// Released samples after a click or hold before the next action, enough
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

/// One tick of a script, with the switch and what the torch sees.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sample {
    pub pressed: bool,
    pub scene: Scene,
}

/// The switch and the scene, one sample per tick, for a whole script.
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
            // The last word's hold is never let go.
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

/// `n` quick clicks, each a press then a short gap, one sample per tick.
pub fn clicks(n: u8) -> impl Iterator<Item = bool> {
    (0..n).flat_map(|_| span(true, PRESS_TICKS).chain(span(false, GAP_TICKS)))
}

fn span(pressed: bool, ticks: u16) -> impl Iterator<Item = bool> {
    iter::repeat_n(pressed, ticks.into())
}

/// The library's torch wired to an [`Adc`], the way `src/main.rs` wires it to
/// the real one, with the result of last tick's conversion in and this tick's
/// conversion out.
#[derive(Clone, Debug)]
pub struct Rig {
    pub torch: Torch,
    /// What [`Rig::tick`] reads. [`Rig::tick_in`] reads a scene instead.
    pub adc: Adc,
}

impl Default for Rig {
    /// A torch whose power-on checks passed, with a good cell at room
    /// temperature.
    fn default() -> Self {
        Self::new(Faults::NONE)
    }
}

impl Rig {
    /// A torch that's just had its cell put in, `faults` being what its
    /// power-on checks found.
    pub const fn new(faults: Faults) -> Self {
        Self {
            torch: Torch::new(CALIBRATION, faults),
            adc: Adc::new(),
        }
    }

    /// One tick, the ADC reading `adc`'s `voltage` and `temperature`.
    pub fn tick(&mut self, pressed: bool) -> Option<Event> {
        let event = self.torch.tick(pressed, self.adc.result());
        self.adc.start(self.torch.convert());
        event
    }

    /// One tick, the ADC reading what `scene` says.
    pub fn tick_in(&mut self, pressed: bool, scene: &Scene) -> Option<Event> {
        let event = self.torch.tick(pressed, self.adc.result());
        self.adc
            .start_with(self.torch.convert(), |channel| scene.reading(channel));
        event
    }

    /// Plays a script on a fresh torch, whose power-on checks find what the
    /// first tick's scene says.
    pub fn play(actions: impl IntoIterator<Item = Action>) -> Self {
        Self::play_with(actions, |_, _| {})
    }

    /// [`Rig::play`], calling `each` after every tick with the click or hold
    /// it finished.
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

/// How the light should be at the end of an example.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Expect {
    Off,
    /// At any level.
    On,
    Level(u8),
}

impl Expect {
    pub fn holds(self, level: u8) -> bool {
        match self {
            Self::Off => level == 0,
            Self::On => level > 0,
            Self::Level(want) => level == want,
        }
    }
}

impl FromStr for Expect {
    type Err = ();

    /// `off`, `on` or `level 50`.
    fn from_str(text: &str) -> Result<Self, ()> {
        match text {
            "off" => Ok(Self::Off),
            "on" => Ok(Self::On),
            _ => {
                let level = text.strip_prefix("level ").ok_or(())?;
                level.parse().map(Self::Level).map_err(drop)
            }
        }
    }
}

/// Which row of docs/ui.md, by its section and its Action (or When) cell.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Row<'a> {
    pub section: &'a str,
    pub action: &'a str,
}

impl fmt::Display for Row<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.section, self.action)
    }
}

/// A row's example, a script and how the light should be on its last tick.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Example<'a> {
    /// The line it's on, counting from 1.
    pub line: usize,
    pub row: Row<'a>,
    /// The script, which [`examples`] has already read once.
    pub script: &'a str,
    pub expect: Expect,
}

impl<'a> Example<'a> {
    pub fn actions(&self) -> impl Iterator<Item = Action> + use<'a> {
        parse(self.script.split_whitespace())
            .map(|action| action.expect("examples read it when it found the row"))
    }
}

/// A line of docs/ui.md that isn't what [`examples`] expects.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SpecError<'a> {
    /// The line, counting from 1.
    pub line: usize,
    pub problem: Problem<'a>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Problem<'a> {
    /// A table with an Action column and no Example column.
    NoExampleColumn,
    /// A row that isn't Planned with no example.
    NoExample(Row<'a>),
    /// An example that isn't `` `script` → off ``, `on` or `level N`.
    Shape(Row<'a>, &'a str),
    /// A script with a word it can't read.
    Script(Row<'a>, ScriptError<'a>),
    /// A row with a different number of cells from its header, most likely
    /// two rows run together, whose second example would go unread.
    Cells { found: usize, header: usize },
}

impl fmt::Display for SpecError<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.problem {
            Problem::NoExampleColumn => f.write_str("this table of actions has no Example column"),
            Problem::NoExample(row) => write!(f, "{row} has no example"),
            Problem::Shape(row, example) => write!(
                f,
                "{row}: expected `script` → off, on or level N, not {example:?}"
            ),
            Problem::Script(row, error) => write!(f, "{row}: {error}"),
            Problem::Cells { found, header } => {
                write!(f, "this row has {found} cells and its header has {header}")
            }
        }
    }
}

/// Every example in a UI spec written like docs/ui.md, in order, or where
/// it isn't.
pub fn examples(markdown: &str) -> Examples<'_> {
    Examples {
        lines: markdown.lines().enumerate(),
        section: "",
        header: None,
    }
}

/// The iterator [`examples`] returns.
#[derive(Clone, Debug)]
pub struct Examples<'a> {
    lines: iter::Enumerate<Lines<'a>>,
    section: &'a str,
    /// The header of the table being read, if any.
    header: Option<&'a str>,
}

impl<'a> Iterator for Examples<'a> {
    type Item = Result<Example<'a>, SpecError<'a>>;

    fn next(&mut self) -> Option<Self::Item> {
        for (number, text) in self.lines.by_ref() {
            let line = number + 1;
            let fail = |problem| Some(Err(SpecError { line, problem }));
            let Some(cells) = text.trim().strip_prefix('|') else {
                self.header = None;
                if let Some(heading) = text.strip_prefix("## ") {
                    self.section = heading.trim();
                }
                continue;
            };
            let Some(header) = self.header else {
                if has(cells, "Action") && !has(cells, "Example") {
                    return fail(Problem::NoExampleColumn);
                }
                self.header = Some(cells);
                continue;
            };
            if split(cells).all(|c| c.starts_with('-') || c.starts_with(':')) {
                continue;
            }
            let (found, header_cells) = (split(cells).count(), split(header).count());
            if found != header_cells {
                return fail(Problem::Cells {
                    found,
                    header: header_cells,
                });
            }
            let cell = |name| {
                let column = split(header).position(|c| c == name)?;
                split(cells).nth(column)
            };
            let Some(example) = cell("Example") else {
                continue;
            };
            let row = Row {
                section: self.section,
                action: cell("Action").or_else(|| cell("When")).unwrap_or("?"),
            };
            if example.is_empty() || example == "-" {
                if cell("Status") == Some("Planned") {
                    continue;
                }
                return fail(Problem::NoExample(row));
            }
            let Some((script, expect)) = read(example) else {
                return fail(Problem::Shape(row, example));
            };
            if let Some(error) = parse(script.split_whitespace()).find_map(Result::err) {
                return fail(Problem::Script(row, error));
            }
            return Some(Ok(Example {
                line,
                row,
                script,
                expect,
            }));
        }
        None
    }
}

/// A table row's cells, minus the `|` it starts with.
fn split(cells: &str) -> impl Iterator<Item = &str> {
    cells.trim_end_matches('|').split('|').map(str::trim)
}

fn has(cells: &str, name: &str) -> bool {
    split(cells).any(|c| c == name)
}

/// `` `1C 2H` → off `` as the script and what to expect.
fn read(example: &str) -> Option<(&str, Expect)> {
    let (script, expect) = example.split_once('→')?;
    let script = script.trim().strip_prefix('`')?.strip_suffix('`')?;
    // At least one word.
    script.split_whitespace().next()?;
    Some((script, expect.trim().parse().ok()?))
}

#[cfg(test)]
mod tests {
    extern crate std;

    use super::*;
    use std::string::ToString;

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

    /// The slow way, with every raw reading tried, the closest winning and the
    /// smallest winning a tie.
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

    /// The torch tests used to read `cell:3.8V` as 0.38 V, dropping the dot.
    #[test]
    fn a_cell_keeps_its_decimal_point() {
        assert_eq!(word("cell:3.8V"), Ok(Action::Cell(380)));
        assert_eq!(word("cell:3.05V"), Ok(Action::Cell(305)));
        assert_eq!(word("cell:4V"), Ok(Action::Cell(400)));
    }

    /// docs/ui.md's rows for a cell the firmware can't read lean on this.
    /// There's no word for a broken reading, only a cell that can't be real.
    #[test]
    fn a_cell_outside_the_plausible_range_reads_as_one_the_firmware_cant_believe() {
        assert_eq!(word("cell:0V"), Ok(Action::Cell(0)));
        // What a dead ADC gives.
        assert_eq!(voltage_reading(0), 0);
        // What the firmware makes of the cell a script sets.
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

    #[test]
    fn a_rig_starts_off_and_plays_a_script() {
        assert_eq!(Rig::default().level(), 0);
        let script = parse(["1C"]).map(Result::unwrap);
        assert_eq!(Rig::play(script).level(), crate::ui::DEFAULT_LEVEL);
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
        let rig = Rig::play(parse(["fault", "2C"]).map(Result::unwrap));
        assert_eq!(rig.level(), crate::limits::FAULT_CEILING);
    }

    const SPEC: &str = "\
## Off

| Action | Does | Example | Status |
|---|---|---|---|
| 1C | On | `1C` → level 50 | Done |
| 2C | Nothing | `2C 1H:2s` → off | Done |
| 3C | Battery | - | Planned |

## Background

| What | Does | Status |
|---|---|---|
| Sleep | Sleeps | Planned |
";

    fn problem(spec: &str) -> Option<SpecError<'_>> {
        examples(spec).find_map(Result::err)
    }

    #[test]
    fn reads_examples_from_tables_with_an_example_column() {
        let mut examples = examples(SPEC).map(Result::unwrap);
        let first = examples.next().unwrap();
        assert_eq!((first.line, first.row.to_string()), (5, "Off: 1C".into()));
        assert!(first.actions().eq([Action::Clicks(1)]));
        assert_eq!(first.expect, Expect::Level(50));
        let second = examples.next().unwrap();
        assert_eq!(second.row.to_string(), "Off: 2C");
        assert_eq!(second.script, "2C 1H:2s");
        assert_eq!(second.expect, Expect::Off);
        assert_eq!(examples.next(), None);
    }

    #[test]
    fn refuses_a_done_row_without_an_example() {
        let spec = SPEC.replace("| 2C | Nothing | `2C 1H:2s` → off |", "| 2C | Nothing | |");
        let error = problem(&spec).unwrap();
        assert_eq!(error.line, 6);
        assert_eq!(error.to_string(), "Off: 2C has no example");
    }

    #[test]
    fn refuses_two_rows_run_together() {
        let spec = SPEC.replace(
            "| 2C | Nothing | `2C 1H:2s` → off |",
            "| 2C | Nothing | `2C 1H:2s` → off | 3C | Nothing | `3C` → off |",
        );
        let error = problem(&spec).unwrap();
        assert_eq!(error.line, 6);
        assert_eq!(
            error.problem,
            Problem::Cells {
                found: 7,
                header: 4
            }
        );
    }

    #[test]
    fn refuses_a_table_of_actions_without_examples() {
        let spec = "| Action | Does | Status |\n|---|---|---|\n| 1C | On | Done |\n";
        let error = problem(spec).unwrap();
        assert_eq!((error.line, error.problem), (1, Problem::NoExampleColumn));
    }

    #[test]
    fn refuses_examples_it_cant_read() {
        for example in [
            "1C → on",
            "`1C` → bright",
            "`1C` on",
            "`` → on",
            "`1C` → level fifty",
        ] {
            assert_eq!(read(example), None, "{example:?} read");
        }
        let spec = SPEC.replace("`1C` → level 50", "`1X` → on");
        let error = problem(&spec).unwrap();
        assert!(matches!(error.problem, Problem::Script(..)), "{error}");
        assert!(error.to_string().starts_with("Off: 1C: `1X`: "), "{error}");
    }

    #[test]
    fn checks_the_light() {
        assert!(Expect::Off.holds(0) && !Expect::Off.holds(1));
        assert!(Expect::On.holds(150) && !Expect::On.holds(0));
        assert!(Expect::Level(50).holds(50) && !Expect::Level(50).holds(51));
    }
}
