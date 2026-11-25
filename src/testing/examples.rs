use super::{Action, parse};
use Expect::{Level, Off};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Expect {
    Off,
    Level(u8),
}

impl Expect {
    #[must_use]
    pub const fn holds(self, level: u8) -> bool {
        match self {
            Self::Off => level == 0,
            Self::Level(want) => level == want,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Example {
    pub name: &'static str,
    pub script: &'static str,
    pub expect: Expect,
}

impl Example {
    #[expect(
        clippy::expect_used,
        clippy::missing_panics_doc,
        reason = "a test parses every script in EXAMPLES"
    )]
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
    ex("Off: 1C", "1C", Level(52)),
    ex("Off: 1C, after using it", "1C 1H 1C 1C", Level(52)),
    ex("Off: 2C or more", "3C", Level(52)),
    ex("Off: 2C or more, then 1C", "3C 1C", Off),
    ex("Off: 2C or more, then 1C twice", "3C 1C 1C", Level(52)),
    ex("Off: 1H", "1H", Level(17)),
    ex("Off: 1H, held for a minute", "1H:60s", Level(17)),
    ex("Off: 1H, let go", "1H wait:10s", Level(17)),
    ex("Off: 1H, then 1H", "1H 1H:1s", Level(56)),
    ex("Off: 1H, then 1H twice", "1H 1H:1s 1H:500ms", Level(50)),
    ex("Off: 1H, then 1C", "1H 1C", Off),
    ex("Off: 1C, after 1H", "1H 1C 1C", Level(52)),
    ex("Off: 2H", "2H:2s wait:1s", Level(52)),
    ex("Rest: 1C", "wait:2s 1C", Level(52)),
    ex("Rest: 1C, on a rest tick", "wait:1984ms 1C", Level(52)),
    ex("Rest: 1H", "wait:2s 1H", Level(17)),
    ex("Rest: 3C", "wait:2s 3C", Level(52)),
    ex("Rest: 3C, locked", "1C 3C wait:10s 3C 1C", Level(52)),
    ex("Rest: 1C, locked", "1C 3C wait:10s 1C", Off),
    ex(
        "Rest: 1C, after a check of the cell",
        "wait:400s 1C",
        Level(52),
    ),
    ex("On: 1C", "1C 1C", Off),
    ex("On: 1H", "1C 1H:3s", Level(150)),
    ex("On: 1H again", "1C 1H:3s 1H:3s", Level(17)),
    ex(
        "On: 1H, after one that stopped partway",
        "1C 1H:1s 1H:500ms",
        Level(85),
    ),
    ex(
        "On: 1H, after turning it off and on",
        "1C 1H:3s 1C 1C 1H:3s",
        Level(150),
    ),
    ex(
        "On: More clicks, or a hold after clicks",
        "1C 1H:1s 2C",
        Off,
    ),
    ex("On: 2C", "1C 2C", Off),
    ex("On: 2C, then 1C", "1C 2C 1C", Level(52)),
    ex("On: 3C", "1C 3C 1C", Off),
    ex("On: 3C, after ramping", "1C 1H:3s 3C 1H", Off),
    ex("On: 4C", "1C 4C 1C", Level(52)),
    ex("On: 2H", "1C 2H:2s", Off),
    ex("Lockout: 3C", "1C 3C 3C", Off),
    ex("Lockout: 3C, then 1C", "1C 3C 3C 1C", Level(52)),
    ex(
        "Lockout: Any other click, or a hold",
        "1C 3C wait:10s 2C",
        Off,
    ),
    ex("Lockout: 1H", "1C 3C 1H:2s", Off),
    ex(
        "Lockout: Anything else",
        "1C 3C 1C 2C 4C 2H:2s wait:1s",
        Off,
    ),
    ex("Low cell", "1C cell:2.95V wait:12s", Level(37)),
    ex("Low cell, for long enough", "1C cell:2.95V wait:60s", Off),
    ex("Low cell, 1H", "1H cell:2.95V wait:3s", Level(17)),
    ex(
        "Low cell, 1H, for long enough",
        "1H cell:2.95V wait:60s",
        Off,
    ),
    ex("Flat cell", "1C cell:2.85V wait:10s", Off),
    ex("Flat cell, 1C again", "1C cell:2.85V wait:10s 1C", Off),
    ex("Flat cell, 1H", "1C cell:2.85V wait:10s 1H", Off),
    ex(
        "Flat cell, rested but not charged, 1C",
        "1C cell:2.85V wait:10s cell:3.20V 1C",
        Off,
    ),
    ex(
        "Flat cell, charged, 1C",
        "1C cell:2.85V wait:10s cell:3.80V 1C",
        Level(52),
    ),
    ex(
        "Flat cell, charged, left alone",
        "1C cell:2.85V wait:10s cell:3.80V wait:301s",
        Off,
    ),
    ex("Cell under 2.80V, lit", "1C cell:2.75V wait:750ms", Off),
    ex(
        "Cell under 2.80V, lit, 1C",
        "1C cell:2.75V wait:750ms 1C",
        Off,
    ),
    ex("Weak cell, 1C", "cell:2.95V 1C", Off),
    ex("Weak cell, 1H", "cell:2.95V 1H", Off),
    ex(
        "Weak cell, charged, 1C",
        "cell:2.95V 1C cell:3.80V 1C",
        Level(52),
    ),
    ex("Low cell, at rest", "cell:3.20V wait:1s 1C", Level(52)),
    ex("Low cell, read by a press", "wait:1s cell:3.20V 1C 1C", Off),
    ex(
        "Low cell, charged at rest",
        "cell:3.20V wait:1s cell:3.80V wait:301s",
        Off,
    ),
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
    ex("Hot", "heat:80C 1C 1H:3s wait:120s", Level(50)),
    ex(
        "Hot, at a level under 50",
        "heat:80C 1C 1H:500ms 1H:3s wait:120s",
        Level(17),
    ),
    ex(
        "Hot, then cool",
        "heat:80C 1C 1H:3s wait:120s heat:25C wait:120s",
        Level(150),
    ),
    ex("A failed check at power-on", "fault 1C", Off),
    ex("A failed check at power-on, 1H", "fault 1H", Off),
    ex(
        "Flash that doesn't match its checksum",
        "flash:corrupt 1C",
        Off,
    ),
];

#[cfg(test)]
#[must_use]
#[expect(
    clippy::missing_panics_doc,
    reason = "tests only, where a name with no example should fail the test"
)]
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
