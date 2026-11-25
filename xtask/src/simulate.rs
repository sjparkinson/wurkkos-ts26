//! Terminals don't say when a key goes up, which a hold needs, so live mode's
//! `h` holds the switch down until pressed again.

use std::collections::VecDeque;
use std::fmt;
use std::thread;
use std::time::{Duration, Instant};

use ts26::TICKS_PER_SECOND;
use ts26::button::{Event, HOLD_TICKS};
use ts26::ramp::LEVELS;
use ts26::testing::{self, Rig, Scene, clicks};

pub use ts26::testing::Action;

use crate::int;
use crate::terminal::{CTRL_C, Terminal};

const PER_SECOND: u64 = int::u64_from_u16(TICKS_PER_SECOND);

pub fn parse(words: &[&str]) -> Result<Vec<Action>, String> {
    testing::parse(words.iter().copied())
        .collect::<Result<_, _>>()
        .map_err(|e| e.to_string())
}

pub struct Run {
    pub lines: Vec<String>,
    pub ticks: u32,
    pub level: u8,
}

pub fn run(actions: &[Action]) -> Run {
    let mut log = Log::default();
    let mut lines = Vec::new();
    let mut ticks = 0_u32;
    let rig = Rig::play_with(actions.iter().copied(), |rig, event| {
        lines.extend(log.note(ticks, event, rig.level()));
        ticks = ticks.saturating_add(1);
    });
    lines.extend(log.settle());
    Run {
        lines,
        ticks,
        level: rig.level(),
    }
}

pub fn script(words: &[&str]) -> Result<(), String> {
    let run = run(&parse(words)?);
    for line in &run.lines {
        println!("{line}");
    }
    println!("{}  ends {}", Seconds(run.ticks), Level(run.level));
    Ok(())
}

#[derive(Default)]
struct Log {
    level: u8,
    hold: Option<(u32, u8)>,
    fade: Option<Fade>,
}

struct Fade {
    start: u32,
    last: u32,
    from: u8,
    to: u8,
}

impl Fade {
    const fn continues(&self, now: u32, before: u8, level: u8) -> bool {
        matches!(now.checked_sub(self.last), Some(1))
            && self.to == before
            && (level > before) == (self.to > self.from)
    }
}

impl Log {
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
        let over = Seconds(last.abs_diff(start).saturating_add(1)).to_string();
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
                let start = now
                    .saturating_sub(u32::from(HOLD_TICKS))
                    .saturating_sub(u32::from(ticks));
                self.hold.get_or_insert((start, before));
                return None;
            }
            Some(Event::Release { clicks }) => {
                let (start, before) = self.hold.take().unwrap_or((now, before));
                let held = Seconds(now.abs_diff(start)).to_string();
                (
                    format!("{}H, held {}", u16::from(clicks) + 1, held.trim_start()),
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

struct Seconds(u32);

impl fmt::Display for Seconds {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let ticks = u64::from(self.0);
        let hundredths = ticks * 100 / PER_SECOND;
        write!(f, "{:>3}.{:02} s", ticks / PER_SECOND, hundredths % 100)
    }
}

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

const BAR: usize = 20;

pub fn interactive() -> Result<(), String> {
    let mut terminal = Terminal::open()?;
    let mut live = Live::default();
    let print = |terminal: &Terminal, text: &str| {
        for line in text.lines() {
            terminal.write(&format!("\r\x1b[2K{line}\n"));
        }
    };
    print(&terminal, &format!("{HELP}\n"));

    let period = Duration::from_micros(1_000_000 / PER_SECOND);
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
        next = next
            .checked_add(period)
            .ok_or("the next tick is past the end of the clock")?;
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

#[derive(Default)]
struct Live {
    torch: Rig,
    log: Log,
    now: u32,
    queue: VecDeque<bool>,
    held: bool,
    pressed: bool,
}

impl Live {
    fn key(&mut self, key: char) -> Command {
        match key.to_ascii_lowercase() {
            'q' | CTRL_C => return Command::Quit,
            ' ' => self.click(1),
            digit @ '1'..='9' => {
                if let Some(n) = digit.to_digit(10).and_then(|n| u8::try_from(n).ok()) {
                    self.click(n);
                }
            }
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

    fn tick(&mut self) -> Option<String> {
        self.pressed = self.queue.pop_front().unwrap_or(self.held);
        let event = self.torch.tick_in(self.pressed, &Scene::NORMAL);
        let lines = self.log.note(self.now, event, self.torch.level());
        self.now = self.now.saturating_add(1);
        (!lines.is_empty()).then(|| lines.join("\n"))
    }

    fn status(&self, width: usize) -> String {
        let level = self.torch.level();
        let output = self.torch.output();
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
            self.torch.aux()
        );
        let text: String = text.chars().take(width.saturating_sub(BAR + 1)).collect();
        format!(
            "\x1b[33m{}\x1b[2m{}\x1b[0m{text}",
            "█".repeat(lit),
            "░".repeat(BAR.saturating_sub(lit))
        )
    }
}

fn lit(level: u8) -> usize {
    (usize::from(level) * BAR).div_ceil(usize::from(LEVELS))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ts26::ui::ON_LEVEL as LEVEL;

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
    fn corrupt_flash_goes_first_and_keeps_the_light_off() {
        assert_eq!(script("flash:corrupt 1C").level, 0);
        let err = parse(&["wait:1s", "flash:corrupt"]).unwrap_err();
        assert!(err.contains("power-on"), "{err}");
    }

    #[test]
    fn a_flat_cell_turns_it_off_and_a_fault_keeps_it_off() {
        assert_eq!(script("cell:2.80V 1C wait:10s").level, 0);
        assert_eq!(script("fault 1C").level, 0);
    }

    #[test]
    fn a_cell_it_cant_read_fades_it_out_for_good() {
        let run = script("cell:0V 1C wait:5s");
        assert_eq!(run.level, 0);
        let [.., fade] = &run.lines[..] else {
            panic!("{:#?}", run.lines)
        };
        let (_, event) = fade.split_once(" s ").unwrap();
        assert!(event.trim_start().starts_with("fade from "), "{fade}");
        assert!(event.contains(" → off over "), "{fade}");
        assert_eq!(script("cell:0V 1C wait:5s cell:3.8V 1C").level, 0);
    }

    #[test]
    fn a_click_turns_it_on_and_another_turns_it_off() {
        assert_eq!(script("1C").level, LEVEL);
        assert_eq!(script("1C 1C").level, 0);
    }

    #[test]
    fn everything_else_leaves_it_alone() {
        assert_eq!(script("1C 3C 5C 3H:2s").level, 0);
        assert_eq!(script("1C 2C").level, 0);
        assert_eq!(script("3C").level, LEVEL);
    }

    #[test]
    fn logs_one_line_per_click_and_per_hold() {
        let lines = script("1C 3C 3H:1s 2C").lines;
        assert_eq!(lines.len(), 5, "{lines:#?}");
        assert!(lines[0].contains("1C") && lines[0].ends_with(&Level(LEVEL).to_string()));
        assert!(lines[1].contains("1C"), "{lines:#?}");
        assert!(lines[2].contains("3C") && lines[2].ends_with("(no change)"));
        assert!(lines[3].contains("3H, held") && lines[3].ends_with("(no change)"));
        assert!(lines[4].contains("2C") && lines[4].ends_with("(no change)"));
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
        assert_eq!(lines.len(), 2, "{lines:?}");
        assert!(lines[0].contains("1C") && lines[1].contains("3C"));
        assert_eq!(live.torch.level(), 0);
        live.key(' ');
        assert_eq!(ticks_of(&mut live, 64).len(), 1);
        assert_eq!(live.torch.level(), 0, "locked");
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
