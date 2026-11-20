//! What every click and hold does, as scripts and how the light ends up.
//! This is the UI spec. `src/ui.rs` implements it, `cargo test` plays each
//! example on the library, and `cargo xtask hex` on the firmware too, so
//! change it in the same commit as the code.
//!
//! The notation is Anduril's (see the module above for every word). A press
//! becomes a hold after about 375 ms, and clicks count once the switch has
//! been left alone for about 220 ms. When more clicks wouldn't mean
//! anything, the last click counts as it's let go, with no wait. That's 1C
//! while it's on, and the third click of 3C while it's off. Lockout always
//! waits, so you unlock with a pause after 3C.
//!
//! Every example starts from a torch that's just had its battery put in. A
//! hold lasts a second unless it says otherwise (`1H:3s`), and a script that
//! ends in a hold never lets go, so it shows the light while it's still
//! held.

use super::{Action, parse};
use Expect::{Level, Off};

/// How the light should be on an example's last tick.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Expect {
    Off,
    Level(u8),
}

impl Expect {
    pub const fn holds(self, level: u8) -> bool {
        match self {
            Self::Off => level == 0,
            Self::Level(want) => level == want,
        }
    }
}

/// A script, played on a torch that's just had its cell put in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Example {
    pub name: &'static str,
    pub script: &'static str,
    pub expect: Expect,
}

impl Example {
    pub fn actions(&self) -> impl Iterator<Item = Action> + use<> {
        parse(self.script.split_whitespace())
            .map(|action| action.expect("every script in EXAMPLES parses"))
    }
}

const fn ex(name: &'static str, script: &'static str, expect: Expect) -> Example {
    Example {
        name,
        script,
        expect,
    }
}

pub const EXAMPLES: &[Example] = &[
    // Off. 1C is on at level 52, every time, since nothing's remembered. 3C
    // locks as the third click's let go, and the clicks after it count in
    // lockout, where they do nothing.
    ex("Off: 1C", "1C", Level(52)),
    ex("Off: 1C, after using it", "1C 1H 1C 1C", Level(52)),
    ex("Off: 3C", "3C 1C", Off),
    ex("Off: 4C or more", "5C 1C", Off),
    ex("Off: Anything else", "2C 1H:2s 2H:2s wait:1s", Off),
    // On. 1C is off as it's let go. The ramp moves a level per 64 Hz tick,
    // so bottom to top takes about 1.7 seconds. Each 1H goes the other way
    // from the one before it, whether or not that one reached the end, and
    // the first after a click on goes up. The top is `ramp::CEILING`, level
    // 120 of 150, until thermal regulation has been checked on a real
    // light, and the bottom is level 13, the first on this curve to reach
    // the stock firmware's smooth floor. Nobody's measured it on this
    // firmware.
    ex("On: 1C", "1C 1C", Off),
    ex("On: 1H", "1C 1H:3s", Level(120)),
    ex("On: 1H again", "1C 1H:3s 1H:3s", Level(13)),
    ex(
        "On: 1H, after one that stopped partway",
        "1C 1H:1s 1H:500ms",
        Level(84),
    ),
    ex(
        "On: 1H, after turning it off and on",
        "1C 1H:3s 1C 1C 1H:3s",
        Level(120),
    ),
    // The first click turns it off without waiting, and the rest count from
    // off. So 2C is off and back on at level 52, 4C is off and locked, and
    // 2H is off and then a hold that does nothing.
    ex(
        "On: More clicks, or a hold after clicks",
        "1C 1H:1s 2C",
        Level(52),
    ),
    ex("On: 4C", "1C 4C 1C", Off),
    ex("On: 2H", "1C 2H:2s", Off),
    // Lockout, for pockets and bags. The light never comes on. 3C from off
    // locks, with the aux bright red for five seconds, and 3C unlocks, back
    // to off. Any other click, or the start of a hold, does nothing but
    // light the red for five seconds again.
    ex("Lockout: 3C, from off", "3C", Off),
    ex("Lockout: 3C", "3C 3C 1C", Level(52)),
    ex("Lockout: Any other click, or a hold", "3C wait:10s 2C", Off),
    ex("Lockout: 1H", "3C 1H:2s", Off),
    ex("Lockout: Anything else", "3C 1C 2C 4C 2H:2s wait:1s", Off),
    // When something's wrong, the light looks after the cell and itself
    // whatever's been asked for, and never flashes to say so. Every
    // threshold behind these is unmeasured on this light.
    //
    // A low cell steps down, and one under 3.00 V for long enough that the
    // steps reach level 1 is flat. So is one under 2.90 V at level 1. A
    // flat cell fades down the ramp and goes off, and after that clicks and
    // holds only light the aux yellow for three seconds. A press reads the
    // cell before its click counts, with the light off, and a reading of
    // 3.60 V or more (a cell that's been charged or swapped) turns it on as
    // if the torch were new. The check every five minutes at rest takes the
    // same reading, so a charged cell is good again with no press, and
    // off's blue comes back.
    ex("Low cell", "cell:2.95V 1C wait:6s", Level(37)),
    ex("Low cell, for long enough", "cell:2.95V 1C wait:60s", Off),
    ex("Flat cell", "cell:2.80V 1C wait:10s", Off),
    ex("Flat cell, 1C again", "cell:2.80V 1C wait:10s 1C", Off),
    ex(
        "Flat cell, rested but not charged, 1C",
        "cell:2.80V 1C wait:10s cell:3.20V 1C",
        Off,
    ),
    ex(
        "Flat cell, charged, 1C",
        "cell:2.80V 1C wait:10s cell:3.80V 1C",
        Level(52),
    ),
    ex(
        "Flat cell, charged, left alone",
        "cell:2.80V 1C wait:10s cell:3.80V wait:301s",
        Off,
    ),
    // Off's dim blue goes dark at the first reading under 3.30 V with the
    // light off, a few ticks after power-on, within five minutes of rest,
    // or with a press. It comes back at the first such reading of 3.40 V or
    // more, and one between the two changes nothing. It's only the aux: the
    // light still comes on with a low cell.
    ex("Low cell, at rest", "cell:3.20V wait:1s 1C", Level(52)),
    ex("Low cell, read by a press", "wait:1s cell:3.20V 2C", Off),
    ex(
        "Low cell, charged at rest",
        "cell:3.20V wait:1s cell:3.80V wait:301s",
        Off,
    ),
    // A cell it can't read, under 1.00 V or over 4.40 V (which is also what
    // a dead ADC reads), is a fault on the fourth reading in a row, about a
    // second in. A fault fades the light down the ramp, a level a tick, and
    // lasts until the cell comes out, with clicks and holds lighting the aux
    // white for three seconds. At rest it's nothing, and the aux stays as it
    // was, until a press reads it.
    ex("A cell it can't read", "cell:0V 1C wait:5s", Off),
    ex(
        "A cell it can read again",
        "cell:0V 1C wait:5s cell:3.80V 1C",
        Off,
    ),
    ex(
        "A cell it can't read, at rest",
        "cell:0V wait:900s cell:3.80V 1C",
        Level(52),
    ),
    // Heat fades it down, no further than level 50, and the level asked for
    // comes back once it's cool.
    ex("Hot", "heat:80C 1C 1H:3s wait:120s", Level(50)),
    ex(
        "Hot, at a level under 50",
        "heat:80C 1C 1H:500ms 1H:3s wait:120s",
        Level(13),
    ),
    ex(
        "Hot, then cool",
        "heat:80C 1C 1H:3s wait:120s heat:25C wait:120s",
        Level(120),
    ),
    // A failed check (flash that doesn't match its checksum, a fuse that
    // isn't what the firmware expects, or a watchdog reset) is a fault too.
    ex("A failed check at power-on", "fault 1C", Off),
    ex(
        "Flash that doesn't match its checksum",
        "flash:corrupt 1C",
        Off,
    ),
];

/// The example called `name`.
pub fn example(name: &str) -> &'static Example {
    EXAMPLES
        .iter()
        .find(|example| example.name == name)
        .unwrap_or_else(|| panic!("no example called {name:?}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_example_parses_and_has_its_own_name() {
        for (i, example) in EXAMPLES.iter().enumerate() {
            let mut words = parse(example.script.split_whitespace()).peekable();
            assert!(words.peek().is_some(), "{}: no script", example.name);
            if let Some(error) = words.find_map(Result::err) {
                panic!("{}: {error}", example.name);
            }
            assert!(
                EXAMPLES[..i].iter().all(|other| other.name != example.name),
                "two examples are called {:?}",
                example.name
            );
        }
    }

    #[test]
    fn checks_the_light() {
        assert!(Expect::Off.holds(0) && !Expect::Off.holds(1));
        assert!(Expect::Level(50).holds(50) && !Expect::Level(50).holds(51));
    }
}
