//! `cargo xtask simulate`: the UI in `src/ui.rs`, run on your machine.
//!
//! It drives `ts26::torch::Torch`, the same loop body the firmware runs, one
//! switch sample per 64 Hz tick. Give it a script in Anduril's notation and
//! it prints what each click and hold did:
//!
//! ```text
//! cargo xtask simulate 1C 2H:1.5s wait:1s 3C
//! ```
//!
//! `ts26::testing` reads the script, the same reader the library's tests of
//! docs/ui.md use, and says what each word means. On the simulated chip,
//! `fault` is a fuse that fails the read-back.
//!
//! With no script it runs live in the terminal, in real time, with the keys
//! in `HELP`. Terminals don't say when a key goes up, which a hold needs, so
//! `h` holds the switch down until pressed again.

use std::collections::VecDeque;
use std::fmt;
use std::thread;
use std::time::{Duration, Instant};

use ts26::TICKS_PER_SECOND;
use ts26::button::{Event, HOLD_TICKS};
use ts26::ramp::{self, LEVELS};
use ts26::testing::{self, clicks};

pub use ts26::testing::Action;

use crate::terminal::{CTRL_C, Terminal};
use crate::world::{Rig, World};

/// Reads a script, word by word.
pub fn parse(words: &[&str]) -> Result<Vec<Action>, String> {
    testing::parse(words.iter().copied())
        .collect::<Result<_, _>>()
        .map_err(|e| e.to_string())
}

/// One tick of a script: the switch, and what the torch sees.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sample {
    pub pressed: bool,
    pub world: World,
}

/// The switch and the world, one sample per tick, for a whole script.
pub fn samples(actions: &[Action]) -> Vec<Sample> {
    testing::samples(actions.iter().copied())
        .map(|sample| Sample {
            pressed: sample.pressed,
            world: World::from(sample.scene),
        })
        .collect()
}

/// What a script did to a fresh torch.
pub struct Run {
    /// One line per click and hold.
    pub lines: Vec<String>,
    pub ticks: u32,
    /// Where the light was left.
    pub level: u8,
}

/// Runs a script on a fresh torch.
pub fn run(actions: &[Action]) -> Run {
    let samples = samples(actions);
    let mut rig = Rig::new(&samples.first().map_or(World::NORMAL, |s| s.world));
    let mut log = Log::default();
    let mut lines = Vec::new();
    for (now, sample) in (0..).zip(&samples) {
        let event = rig.tick(sample.pressed, &sample.world);
        lines.extend(log.note(now, event, rig.level()));
    }
    lines.extend(log.settle());
    Run {
        lines,
        ticks: samples.len() as u32,
        level: rig.level(),
    }
}

/// Runs a script and prints what happened.
pub fn script(words: &[&str]) -> Result<(), String> {
    let run = run(&parse(words)?);
    for line in &run.lines {
        println!("{line}");
    }
    println!("{}  ends {}", Seconds(run.ticks), Level(run.level));
    Ok(())
}

/// Turns ticks into log lines: one per click, one per hold (when it ends,
/// not every tick it's held), and one for any change of level that no
/// click or hold explains. A run of one-level steps a tick apart, all the
/// same way, is a fade, and gets one line when it stops.
#[derive(Default)]
struct Log {
    level: u8,
    /// The tick the press that's being held started on, and the level
    /// before it.
    hold: Option<(u32, u8)>,
    fade: Option<Fade>,
}

/// A fade in progress: the tick of its first and latest steps, and the
/// level before it and now.
struct Fade {
    start: u32,
    last: u32,
    from: u8,
    to: u8,
}

impl Fade {
    /// Whether a step from `before` to `level` on tick `now` carries on.
    fn continues(&self, now: u32, before: u8, level: u8) -> bool {
        self.last + 1 == now && self.to == before && (level > before) == (self.to > self.from)
    }
}

impl Log {
    /// The lines this tick ends up with: usually none, sometimes a fade that
    /// just stopped and the thing that stopped it.
    fn note(&mut self, now: u32, event: Option<Event>, level: u8) -> Vec<String> {
        let before = std::mem::replace(&mut self.level, level);
        let quiet = event.is_none() && self.hold.is_none();
        if quiet && level.abs_diff(before) == 1 {
            if let Some(fade) = &mut self.fade
                && fade.continues(now, before, level)
            {
                fade.last = now;
                fade.to = level;
                return Vec::new();
            }
            let stopped = self.settle();
            self.fade = Some(Fade {
                start: now,
                last: now,
                from: before,
                to: level,
            });
            return stopped.into_iter().collect();
        }
        let mut lines: Vec<String> = self.settle().into_iter().collect();
        lines.extend(self.line(now, event, before, level));
        lines
    }

    /// The line for a fade that's stopped or been interrupted, if there was
    /// one. A single step shows as a plain change of level.
    fn settle(&mut self) -> Option<String> {
        let Fade {
            start,
            last,
            from,
            to,
        } = self.fade.take()?;
        if from.abs_diff(to) == 1 {
            return Some(format!("{}  {:<16} → {}", Seconds(start), "", Level(to)));
        }
        let over = Seconds(last + 1 - start).to_string();
        Some(
            format!(
                "{}  {:<16} → {}",
                Seconds(start),
                format!("fade from {:#}", Level(from)),
                Level(to)
            ) + &format!(" over {}", over.trim_start()),
        )
    }

    fn line(&mut self, now: u32, event: Option<Event>, before: u8, level: u8) -> Option<String> {
        let (what, before) = match event {
            Some(Event::Click(n)) => (format!("{n}C"), before),
            Some(Event::Hold { ticks, .. }) => {
                // The first Hold comes HOLD_TICKS into the press.
                let start = now - u32::from(HOLD_TICKS) - u32::from(ticks);
                self.hold.get_or_insert((start, before));
                return None;
            }
            Some(Event::Release { clicks }) => {
                let (start, before) = self.hold.take().unwrap_or((now, before));
                let held = Seconds(now - start).to_string();
                (
                    format!("{}H, held {}", clicks + 1, held.trim_start()),
                    before,
                )
            }
            None if self.hold.is_none() && level != before => (String::new(), before),
            None => return None,
        };
        let unchanged = if level == before { " (no change)" } else { "" };
        Some(format!(
            "{}  {what:<16} → {}{unchanged}",
            Seconds(now),
            Level(level)
        ))
    }
}

/// Ticks shown as seconds: `  1.50 s` for 96 ticks.
struct Seconds(u32);

impl fmt::Display for Seconds {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let per = u32::from(TICKS_PER_SECOND);
        write!(f, "{:>3}.{:02} s", self.0 / per, self.0 % per * 100 / per)
    }
}

/// A level as people read it: `off` or `level 50/150`, or with `{:#}`, the
/// shorter `50/150`.
struct Level(u8);

impl fmt::Display for Level {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            0 => f.pad("off"),
            n if f.alternate() => f.pad(&format!("{n}/{LEVELS}")),
            n => f.pad(&format!("level {n}/{LEVELS}")),
        }
    }
}

const HELP: &str = "\
space  click
1-9    that many quick clicks
h      hold the switch down, or let it go (terminals can't report a key
       being let go, so holding a key down won't do)
r      start again from off
?      these keys
q      quit";

/// Width of the brightness bar on the status line, in characters.
const BAR: usize = 20;

/// Runs the UI live in the terminal until `q` or Ctrl-C.
pub fn interactive() -> Result<(), String> {
    let mut terminal = Terminal::open()?;
    let mut live = Live::default();
    let print = |terminal: &Terminal, text: &str| {
        for line in text.lines() {
            terminal.write(&format!("\r\x1b[2K{line}\n"));
        }
    };
    print(&terminal, &format!("{HELP}\n"));

    let period = Duration::from_micros(1_000_000 / u64::from(TICKS_PER_SECOND));
    let mut next = Instant::now();
    let mut shown = String::new();
    loop {
        let Some(keys) = terminal.poll() else {
            return Ok(());
        };
        for key in keys {
            match live.key(key) {
                Command::Continue => {}
                Command::Help => print(&terminal, HELP),
                Command::Reset => print(&terminal, "reset"),
                Command::Quit => return Ok(()),
            }
        }
        if let Some(line) = live.tick() {
            print(&terminal, &line);
            shown.clear();
        }
        let status = live.status(terminal.width);
        if status != shown {
            terminal.write(&format!("\r\x1b[2K{status}"));
            shown = status;
        }
        // Keep to the clock, not to how long each pass took.
        next += period;
        thread::sleep(next.saturating_duration_since(Instant::now()));
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Command {
    Continue,
    Help,
    Reset,
    Quit,
}

/// A live session, minus the terminal: keys in, ticks, a status line out.
#[derive(Default)]
struct Live {
    /// The torch, with a 3.8 V cell at 25 °C.
    torch: Rig,
    log: Log,
    now: u32,
    /// Scripted samples from clicks, played before `held` counts.
    queue: VecDeque<bool>,
    /// Where the switch rests once `queue` runs out.
    held: bool,
    /// The last sample the torch saw.
    pressed: bool,
}

impl Live {
    fn key(&mut self, key: char) -> Command {
        match key.to_ascii_lowercase() {
            'q' | CTRL_C => return Command::Quit,
            ' ' => self.click(1),
            digit @ '1'..='9' => self.click(digit as u8 - b'0'),
            'h' => self.held = !self.held,
            '?' => return Command::Help,
            'r' => {
                *self = Self::default();
                return Command::Reset;
            }
            _ => {}
        }
        Command::Continue
    }

    fn click(&mut self, n: u8) {
        self.held = false;
        self.queue.extend(clicks(n));
    }

    /// One 64 Hz tick, and a log line if it did something.
    fn tick(&mut self) -> Option<String> {
        self.pressed = self.queue.pop_front().unwrap_or(self.held);
        let event = self.torch.tick(self.pressed, &World::NORMAL);
        let lines = self.log.note(self.now, event, self.torch.level());
        self.now += 1;
        (!lines.is_empty()).then(|| lines.join("\n"))
    }

    /// A brightness bar, the level and its delta-sigma value, the switch and
    /// the aux colour, cut to fit in `width` columns.
    fn status(&self, width: usize) -> String {
        let level = self.torch.level();
        let output = ramp::output(level);
        let lit = lit(level);
        let switch = match (self.pressed, self.held) {
            (true, true) => "held",
            (true, false) => "down",
            (false, _) => "up",
        };
        let text = format!(
            " {:>#7}  dsm {:>5}  switch {switch:<4}  aux {:?}",
            Level(level),
            output.dsm,
            self.torch.torch.aux()
        );
        let text: String = text.chars().take(width.saturating_sub(BAR + 1)).collect();
        format!(
            "\x1b[33m{}\x1b[2m{}\x1b[0m{text}",
            "█".repeat(lit),
            "░".repeat(BAR - lit)
        )
    }
}

/// How much of the bar a level fills: none when off, at least one block
/// when on, and all of it at the top.
fn lit(level: u8) -> usize {
    (usize::from(level) * BAR).div_ceil(usize::from(LEVELS))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ts26::ui::DEFAULT_LEVEL as LEVEL;

    fn script(script: &str) -> Run {
        let words: Vec<_> = script.split_whitespace().collect();
        run(&parse(&words).unwrap())
    }

    #[test]
    fn says_which_word_it_cant_read_and_why() {
        assert_eq!(
            parse(&["1C", "2H:1.5s", "wait:1s"]),
            Ok(vec![
                Action::Clicks(1),
                Action::Hold {
                    clicks: 1,
                    ticks: 96
                },
                Action::Wait(64),
            ])
        );
        let err = parse(&["1C", "cell:3"]).unwrap_err();
        assert!(err.starts_with("`cell:3`: expected a voltage"), "{err}");
    }

    #[test]
    fn corrupt_flash_goes_first_and_caps_the_light() {
        assert_eq!(script("flash:corrupt 2C").level, 50);
        let err = parse(&["wait:1s", "flash:corrupt"]).unwrap_err();
        assert!(err.contains("power-on"), "{err}");
    }

    #[test]
    fn a_change_of_scene_takes_no_time_and_lasts() {
        let samples = samples(&parse(&["wait:1s", "cell:2.95V", "wait:1s"]).unwrap());
        assert_eq!(samples.len(), 128);
        assert_eq!(samples[63].world.cell, 380);
        assert_eq!(samples[64].world.cell, 295);
    }

    #[test]
    fn a_flat_cell_turns_it_off_and_a_fault_caps_it() {
        assert_eq!(script("cell:2.80V 2C wait:10s").level, 0);
        assert_eq!(script("fault 2C").level, 50);
    }

    #[test]
    fn a_click_turns_it_on_and_another_turns_it_off() {
        assert_eq!(script("1C").level, LEVEL);
        assert_eq!(script("1C 1C").level, 0);
    }

    #[test]
    fn everything_else_leaves_it_alone() {
        assert_eq!(script("3C 5C 3H:2s").level, 0);
        assert_eq!(script("1C 3C 5C").level, LEVEL);
    }

    #[test]
    fn logs_one_line_per_click_and_per_hold() {
        let lines = script("1C 3H:1s 3C").lines;
        assert_eq!(lines.len(), 3, "{lines:#?}");
        assert!(lines[0].contains("1C") && lines[0].ends_with(&Level(LEVEL).to_string()));
        assert!(lines[1].contains("3H, held") && lines[1].ends_with("(no change)"));
        assert!(lines[2].contains("3C") && lines[2].ends_with("(no change)"));
    }

    #[test]
    fn times_a_hold_from_the_start_of_the_press() {
        let mut log = Log::default();
        for ticks in 0..10 {
            let hold = Event::Hold { clicks: 1, ticks };
            let now = u32::from(HOLD_TICKS + ticks);
            assert!(log.note(now, Some(hold), 0).is_empty());
        }
        let now = u32::from(HOLD_TICKS) + 10;
        let line = log
            .note(now, Some(Event::Release { clicks: 1 }), 0)
            .remove(0);
        let held = Seconds(now).to_string();
        assert!(
            line.contains(&format!("2H, held {}", held.trim_start())),
            "{line}"
        );
    }

    #[test]
    fn logs_a_change_of_level_with_no_event() {
        let mut log = Log::default();
        assert!(log.note(0, None, 30)[0].ends_with("level 30/150"));
        assert!(log.note(1, None, 30).is_empty());
        // A lone one-level step shows once the next tick says it's alone.
        assert!(log.note(2, None, 31).is_empty());
        let lines = log.note(3, None, 31);
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(lines[0].starts_with(&Seconds(2).to_string()), "{lines:?}");
        assert!(lines[0].ends_with("level 31/150"), "{lines:?}");
    }

    #[test]
    fn logs_a_fade_as_one_line_when_it_stops() {
        let mut log = Log::default();
        log.note(0, None, 150);
        for (now, level) in (1..).zip((140..150).rev()) {
            assert!(log.note(now, None, level).is_empty(), "tick {now}");
        }
        let lines = log.note(11, None, 140);
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert_eq!(
            lines[0],
            format!(
                "{}  fade from 150/150 → level 140/150 over 0.15 s",
                Seconds(1)
            )
        );
    }

    #[test]
    fn a_click_or_a_turn_stops_a_fade() {
        let mut log = Log::default();
        log.note(0, None, 10);
        log.note(1, None, 9);
        log.note(2, None, 8);
        // Turning round starts a new fade.
        let lines = log.note(3, None, 9);
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(
            lines[0].contains("fade from 10/150 → level 8/150 "),
            "{lines:?}"
        );
        log.note(4, None, 10);
        let lines = log.note(5, Some(Event::Click(1)), 0);
        assert_eq!(lines.len(), 2, "{lines:?}");
        assert!(
            lines[0].contains("fade from 8/150  → level 10/150 "),
            "{lines:?}"
        );
        assert!(
            lines[1].contains("1C") && lines[1].ends_with("off"),
            "{lines:?}"
        );
        assert_eq!(log.settle(), None);
    }

    /// Ticks a live session `n` times and keeps the log.
    fn ticks_of(live: &mut Live, n: u16) -> Vec<String> {
        (0..n).filter_map(|_| live.tick()).collect()
    }

    #[test]
    fn space_clicks_and_digits_click_that_many_times() {
        let mut live = Live::default();
        live.key(' ');
        let lines = ticks_of(&mut live, 64);
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(lines[0].contains("1C") && live.torch.level() == LEVEL);

        live.key('3');
        let lines = ticks_of(&mut live, 64);
        assert!(lines[0].contains("3C") && live.torch.level() == LEVEL);
    }

    #[test]
    fn h_holds_the_switch_until_pressed_again() {
        let mut live = Live::default();
        live.key('h');
        assert!(ticks_of(&mut live, 64).is_empty());
        assert!(live.pressed);
        live.key('h');
        let lines = ticks_of(&mut live, 64);
        assert!(lines[0].contains("1H, held 1.00 s"), "{lines:?}");
        assert!(!live.pressed);
    }

    #[test]
    fn q_and_ctrl_c_quit_and_r_starts_again() {
        let mut live = Live::default();
        assert_eq!(live.key('q'), Command::Quit);
        assert_eq!(live.key(CTRL_C), Command::Quit);
        assert_eq!(live.key('?'), Command::Help);

        live.key(' ');
        ticks_of(&mut live, 64);
        assert_eq!(live.key('r'), Command::Reset);
        assert_eq!((live.torch.level(), live.now), (0, 0));
    }

    #[test]
    fn the_bar_is_empty_only_when_off_and_full_only_at_the_top() {
        assert_eq!(lit(0), 0);
        assert_eq!(lit(1), 1);
        assert_eq!(lit(LEVELS - 1), BAR);
        assert_eq!(lit(LEVELS), BAR);
        assert_eq!(lit(LEVELS / 2), BAR / 2);
    }

    #[test]
    fn the_status_line_fits_the_terminal() {
        let mut live = Live::default();
        live.key(' ');
        ticks_of(&mut live, 64);
        let status = live.status(60);
        assert!(status.contains(&format!(" {LEVEL}/{LEVELS} ")), "{status}");
        let visible = status.replace("\x1b[33m", "").replace("\x1b[2m", "");
        let visible = visible.replace("\x1b[0m", "");
        assert_eq!(visible.chars().count(), 59);
    }

    #[test]
    fn shows_levels_and_seconds() {
        assert_eq!(Level(0).to_string(), "off");
        assert_eq!(Level(50).to_string(), "level 50/150");
        assert_eq!(format!("{:>#7}", Level(50)), " 50/150");
        assert_eq!(Seconds(0).to_string(), "  0.00 s");
        assert_eq!(Seconds(96).to_string(), "  1.50 s");
        assert_eq!(Seconds(64 * 125 + 1).to_string(), "125.01 s");
    }
}
