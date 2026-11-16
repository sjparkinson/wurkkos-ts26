//! `cargo xtask breakages` breaks the firmware on purpose, one mistake at a
//! time, and checks `cargo xtask hex` refuses each for the reason expected.
//!
//! Firmware that's right trips none of the simulator's alarms, nor
//! `layout.rs`'s, so a `hex` that passes says nothing about whether they can
//! still ring. sim.rs's tests ring the ones a test can reach by driving the
//! fake chip. The rest only go off with the real firmware running, and
//! `BREAKAGES` is a mistake for each kind of those.
//!
//! The checkout is never edited. The source is copied to a directory under
//! the system's temporary one, and `cargo xtask hex` runs there as a child
//! process, so it's the copy's own xtask judging the copy's firmware, built
//! into the copy's own `target`. First the copy as it is, which has to pass,
//! or a refusal further on would mean nothing. Then each breakage in turn,
//! in that one copy, so xtask and `core` are only built once. Each puts back
//! the file the last one broke, makes its one substitution, and wants `hex`
//! to fail at the stage it names with the text it names. The copy goes when
//! the run ends, pass or fail, though not if the run's killed.
//!
//! They run one after another. `hex` already runs its simulations side by
//! side, and a second copy would have to build xtask and `core` again, which
//! takes longer than every breakage put together. Most are refused in under
//! a second, before the simulator gets to its long runs.
//!
//! Like the simulator, this finds mistakes and proves nothing. A breakage
//! that's refused shows one alarm rings for one mistake. It doesn't show the
//! alarm rings for every mistake of that kind, and a mistake nobody thought
//! to list here is as unchecked as it was.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{self, Command};
use std::time::Instant;

/// What gets copied, from the workspace root. It's what `cargo xtask hex`
/// needs to build xtask and the firmware, and nothing else. docs/ui.md is
/// there because the simulator runs its examples.
const COPIED: [&str; 11] = [
    ".cargo",
    "Cargo.lock",
    "Cargo.toml",
    "build.rs",
    "docs/ui.md",
    "examples",
    "link.x",
    "rust-toolchain.toml",
    "src",
    "vendor",
    "xtask",
];

/// Directories that are never copied, wherever they turn up.
const SKIPPED: [&str; 2] = ["target", ".git"];

/// How far `cargo xtask hex` got before it refused the firmware.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stage {
    /// The firmware didn't build. For the breakages here, that's the linker.
    Build,
    /// `layout.rs` refused the linked image.
    Layout,
    /// `sim.rs` refused it, running.
    Simulator,
}

/// One deliberate mistake.
struct Breakage {
    name: &'static str,
    /// The file to break, from the workspace root.
    file: &'static str,
    /// Text that's in `file` exactly once.
    find: &'static str,
    /// What goes in its place.
    replace: &'static str,
    /// Where `hex` has to stop.
    stage: Stage,
    /// Text `hex`'s output has to have in it.
    expect: &'static str,
}

const HW: &str = "src/avr/hw.rs";
const MAIN: &str = "src/main.rs";

/// The mistakes, each one a different alarm. Every one of them has been run
/// and seen refused for the reason it gives, and `cargo xtask breakages`
/// sees to it that stays true.
const BREAKAGES: &[Breakage] = &[
    // Turning on: the duty goes first, then the enable.
    Breakage {
        name: "enable-before-duty",
        file: HW,
        find: "    let was_off = read(reg::VPORTB_OUT) & 1 << ENABLE == 0;\n",
        replace: "    let was_off = read(reg::VPORTB_OUT) & 1 << ENABLE == 0;\n    \
                  write(reg::VPORTB_OUT, read(reg::VPORTB_OUT) | 1 << ENABLE);\n",
        stage: Stage::Simulator,
        expect: "raises PB2 with the duty at 0",
    },
    // Turning off: the enable drops first, then the duty.
    Breakage {
        name: "duty-before-enable",
        file: HW,
        find: "        write(reg::VPORTB_OUT, read(reg::VPORTB_OUT) & !(1 << ENABLE));\n        \
               without_interrupts(|| {\n",
        replace: "        without_interrupts(|| {\n",
        stage: Stage::Simulator,
        expect: "writes TCA0.CMP0BUF from the main loop with PB2 high",
    },
    // Turning off hands PB0 back to PORT, since nothing says what a stopped
    // timer leaves on it.
    Breakage {
        name: "pb0-left-to-tca0",
        file: HW,
        find: "            write(reg::TCA0_CTRLB, CTRLB_OFF);\n",
        replace: "",
        stage: Stage::Simulator,
        expect: "powers down with TCA0.CTRLB.CMP0EN set",
    },
    // Turning off zeroes the live compare too, so power-down can follow on
    // the same pass without waiting for BOTTOM to bring in CMP0BUF's 0.
    Breakage {
        name: "no-direct-compare-write",
        file: HW,
        find: "            write16(reg::TCA0_CMP0, 0);\n",
        replace: "",
        stage: Stage::Simulator,
        expect: "powers down with CMP0 at",
    },
    // A change of level holds interrupts off, so the handler never sees one
    // byte of the new level and one of the old.
    Breakage {
        name: "level-change-with-interrupts-on",
        file: HW,
        find: "    unsafe { asm!(\"cli\", options(nostack)) };\n    f();\n",
        replace: "    f();\n",
        stage: Stage::Simulator,
        expect: "so the handler can catch the level half changed",
    },
    // The delta-sigma starts from nothing each time the light comes on.
    Breakage {
        name: "sum-kept-at-turn-off",
        file: HW,
        find: "            write(DSM_SUM, 0);\n",
        replace: "",
        stage: Stage::Simulator,
        expect: "with the delta-sigma's sum at",
    },
    // Every BOTTOM raises the overflow flag, light on or off, so turning on
    // clears it before the interrupt goes on.
    Breakage {
        name: "stale-overflow-flag",
        file: HW,
        find: "            write(reg::TCA0_INTFLAGS, regs::TCA_OVF);\n",
        replace: "",
        stage: Stage::Simulator,
        expect: "with INTFLAGS.OVF still up from before",
    },
    // `hw::halt` clears SREG.I as well as turning every interrupt off, since
    // the datasheet doesn't say either alone keeps the chip down.
    Breakage {
        name: "halt-without-cli",
        file: HW,
        find: "    unsafe { asm!(\"cli\", options(nostack)) };\n    set_output(Output::OFF);\n",
        replace: "    set_output(Output::OFF);\n",
        stage: Stage::Simulator,
        expect: "stops for good with SREG.I still set",
    },
    // `prepare_sleep` idles rather than power down with PB2 high, whatever
    // it's asked. The firmware never asks, so examples/guards.rs does.
    Breakage {
        name: "power-down-with-the-light-on",
        file: HW,
        find: "    let mode = if rest && read(reg::VPORTB_OUT) & 1 << ENABLE == 0 {\n",
        replace: "    let mode = if rest {\n",
        stage: Stage::Simulator,
        expect: "powers down with PB2 high",
    },
    Breakage {
        name: "pb2-pull-up",
        file: HW,
        find: "        if PORTB_OUTPUTS & 1 << pin == 0 {\n",
        replace: "        if 1 << PWM & 1 << pin == 0 {\n",
        stage: Stage::Simulator,
        expect: "gives PB2 a pull-up",
    },
    // 16-bit registers go low byte first.
    Breakage {
        name: "high-byte-first",
        file: HW,
        find: "    write(addr, lo);\n    write(addr + 1, hi);\n",
        replace: "    write(addr + 1, hi);\n    write(addr, lo);\n",
        stage: Stage::Simulator,
        expect: "writes the high byte of TCA0.PER without its low byte",
    },
    Breakage {
        name: "watchdog-never-fed",
        file: MAIN,
        find: "            hw::feed_watchdog();\n",
        replace: "",
        stage: Stage::Simulator,
        expect: "without wdr",
    },
    Breakage {
        name: "watchdog-fed-every-tick",
        file: MAIN,
        find: "        if feeder.tick() {\n",
        replace: "        if feeder.tick() || true {\n",
        stage: Stage::Simulator,
        expect: "closed window",
    },
    Breakage {
        name: "adc-on-in-power-down",
        file: HW,
        find: "        write(reg::ADC0_CTRLA, 0);\n        write(reg::VREF_CTRLB, 0);\n",
        replace: "        write(reg::VREF_CTRLB, 0);\n",
        stage: Stage::Simulator,
        expect: "powers down with ADC0 still enabled",
    },
    // `hw::halt` is where the firmware ends up when it can't go on, and it
    // has to leave the light off. This one still turns TCA0's interrupt off,
    // so the `sleep` is one the chip would take, with PB2 high.
    Breakage {
        name: "halt-leaves-the-light-on",
        file: HW,
        find: "    set_output(Output::OFF);\n    write(reg::RTC_PITINTCTRL, 0);\n",
        replace: "    write(reg::TCA0_INTCTRL, 0);\n    write(reg::RTC_PITINTCTRL, 0);\n",
        stage: Stage::Simulator,
        expect: "stopped for good with PB2 high",
    },
    // PB2 is cleared in OUT before it's made an output.
    Breakage {
        name: "dir-before-out",
        file: HW,
        find: "    write(reg::VPORTB_OUT, 0);\n    write(reg::VPORTB_DIR, PORTB_OUTPUTS);\n",
        replace: "    write(reg::VPORTB_DIR, PORTB_OUTPUTS);\n    write(reg::VPORTB_OUT, 0);\n",
        stage: Stage::Simulator,
        expect: "starting with PB2 high: the first register it writes leaves PB2 set",
    },
    // The reset flags are read and cleared before anything can reset the
    // chip again.
    Breakage {
        name: "watchdog-before-reset-flags",
        file: MAIN,
        find: "    let faults = Faults {\n        \
               hang: selftest::hung(hw::take_reset_flags()),\n        \
               flash: !hw::flash_ok(),\n        \
               fuses: !hw::fuses().ok(),\n    };\n\n    \
               hw::init();\n    hw::start_watchdog();\n",
        replace: "    hw::init();\n    hw::start_watchdog();\n\n    \
                  let faults = Faults {\n        \
                  hang: selftest::hung(hw::take_reset_flags()),\n        \
                  flash: !hw::flash_ok(),\n        \
                  fuses: !hw::fuses().ok(),\n    };\n",
        stage: Stage::Simulator,
        expect: "start-up order: turns the watchdog on before clearing the reset flags",
    },
    // A handler never touches TCA0's 16-bit registers while the main loop is
    // halfway through one. This moves the main loop's write of the compare
    // to after the handler's turned on and interrupts are back.
    Breakage {
        name: "handler-races-temp",
        file: HW,
        find: "            write16(reg::TCA0_CMP0BUF, u16::from(duty));\n            \
               write(reg::TCA0_CTRLB, CTRLB_ON);\n            \
               write(reg::TCA0_INTFLAGS, regs::TCA_OVF);\n            \
               write(reg::TCA0_INTCTRL, regs::TCA_OVF);\n        }\n    });\n",
        replace: "            write(reg::TCA0_CTRLB, CTRLB_ON);\n            \
                  write(reg::TCA0_INTFLAGS, regs::TCA_OVF);\n            \
                  write(reg::TCA0_INTCTRL, regs::TCA_OVF);\n        }\n    });\n    \
                  if was_off {\n        \
                  write16(reg::TCA0_CMP0BUF, u16::from(duty));\n    }\n",
        stage: Stage::Simulator,
        expect: "share TCA0's TEMP byte",
    },
    // With the light on and no tick, the wait gives up and `main` halts.
    // This count never runs out.
    Breakage {
        name: "no-tick-deadline",
        file: HW,
        find: "            cycles = left;\n",
        replace: "            cycles = left.max(1);\n",
        stage: Stage::Simulator,
        expect: "past the deadline",
    },
    // A protected register's write comes straight after its unlock.
    Breakage {
        name: "unlock-apart-from-its-write",
        file: HW,
        find: "            \"out {ccp}, {key}\",\n",
        replace: "            \"out {ccp}, {key}\",\n            \"nop\",\n",
        stage: Stage::Layout,
        expect: "isn't followed by `st Z`",
    },
    // A handler gives back every register as it found it.
    Breakage {
        name: "handler-clobbers-a-register",
        file: HW,
        find: "    write(reg::TCA0_INTFLAGS, regs::TCA_OVF);\n    let sum =",
        replace: "    write(reg::TCA0_INTFLAGS, regs::TCA_OVF);\n    \
                  unsafe { asm!(\"inc r2\", options(nostack)) };\n    let sum =",
        stage: Stage::Simulator,
        expect: "the handler for TCA0_OVF (vector 8): returns with r2",
    },
    // Nothing in the firmware can panic. This is the check CONTRIBUTING.md
    // wants shown to fire on every new nightly, since it goes by the names
    // of symbols.
    Breakage {
        name: "bounds-check",
        file: MAIN,
        find: "        torch.tick(hw::switch_pressed(), hw::adc_result());\n",
        replace: "        let reading = hw::adc_result();\n        \
                  let pressed = [hw::switch_pressed(); 4][usize::from(reading.unwrap_or(0))];\n        \
                  torch.tick(pressed, reading);\n",
        stage: Stage::Layout,
        expect: "panic_bounds_check",
    },
    // No 32-bit multiply, which dividing a `u16` by ten turns into.
    Breakage {
        name: "wide-multiply",
        file: HW,
        find: "    Some(u16::from_le_bytes([lo, hi]))\n",
        replace: "    Some(u16::from_le_bytes([lo, hi]) / 10)\n",
        stage: Stage::Build,
        expect: "undefined symbol: __mulsi3",
    },
];

/// Runs the breakages named, or all of them with none named.
pub fn run(root: &Path, names: &[&str]) -> Result<(), String> {
    let chosen = choose(names)?;
    let copy = Scratch::of(root)?;
    let started = Instant::now();

    eprintln!("building an unbroken copy in {}", copy.dir.display());
    let (passed, output) = copy.hex()?;
    if !passed {
        return Err(format!(
            "the copy doesn't pass `cargo xtask hex` with nothing broken, so no refusal would \
             mean anything:\n{output}"
        ));
    }
    eprintln!("it passes, in {} s", started.elapsed().as_secs());

    let mut failed = Vec::new();
    for breakage in &chosen {
        let began = Instant::now();
        match copy.refuses(root, breakage) {
            Ok(verdict) => eprintln!(
                "refused {} in {:.1} s: {verdict}",
                breakage.name,
                began.elapsed().as_secs_f32()
            ),
            Err(e) => {
                eprintln!("NOT REFUSED {}: {e}", breakage.name);
                failed.push(breakage.name);
            }
        }
    }
    if !failed.is_empty() {
        return Err(format!(
            "{} of {} breakages got past `cargo xtask hex`, or were refused for some other \
             reason: {}",
            failed.len(),
            chosen.len(),
            failed.join(", ")
        ));
    }
    println!(
        "{} breakages, each refused for its own reason, in {} s",
        chosen.len(),
        started.elapsed().as_secs()
    );
    Ok(())
}

/// The breakages called `names`, in the table's order, or all of them.
fn choose(names: &[&str]) -> Result<Vec<&'static Breakage>, String> {
    if let Some(unknown) = names
        .iter()
        .find(|name| BREAKAGES.iter().all(|b| b.name != **name))
    {
        let known: Vec<&str> = BREAKAGES.iter().map(|b| b.name).collect();
        return Err(format!(
            "no breakage called `{unknown}`; there's {}",
            known.join(", ")
        ));
    }
    Ok(BREAKAGES
        .iter()
        .filter(|b| names.is_empty() || names.contains(&b.name))
        .collect())
}

/// `source` with `find` swapped for `replace`. `find` has to be there exactly
/// once, so a refactor that moves or rewords the line fails here, loudly, and
/// can't turn a breakage into one that changes nothing.
fn substitute(source: &str, find: &str, replace: &str) -> Result<String, String> {
    if find.is_empty() || find == replace {
        return Err("the substitution changes nothing".into());
    }
    match source.matches(find).count() {
        1 => Ok(source.replacen(find, replace, 1)),
        n => Err(format!(
            "the text to break is there {n} times, not once:\n{find}"
        )),
    }
}

/// Where `hex` stopped, and the line it stopped with, from what it printed.
/// xtask's last word is the last line starting `error: `. If that names an
/// ELF, the build worked and the image was refused, by the simulator if it
/// says so and by `layout.rs` if not. If it doesn't, it's cargo having
/// failed, and what the compiler or the linker said is further up.
fn verdict(output: &str) -> Option<(Stage, &str)> {
    let line = output
        .lines()
        .rev()
        .find_map(|line| line.strip_prefix("error: "))?;
    let stage = if line.contains(".elf: simulator: ") {
        Stage::Simulator
    } else if line.contains(".elf: ") {
        Stage::Layout
    } else {
        Stage::Build
    };
    Some((stage, line))
}

/// Whether `output` is `hex` refusing the firmware at `stage` with `expect`
/// in what it said. Gives back the line that has `expect` in it, less the
/// path of the ELF.
fn refused<'a>(output: &'a str, stage: Stage, expect: &str) -> Result<&'a str, String> {
    let Some((stopped, _)) = verdict(output) else {
        return Err(format!("it failed without saying why:\n{output}"));
    };
    if stopped != stage {
        return Err(format!(
            "it stopped at {stopped:?}, not {stage:?}:\n{output}"
        ));
    }
    let Some(line) = output.lines().find(|line| line.contains(expect)) else {
        return Err(format!(
            "it was refused, but not with `{expect}`:\n{output}"
        ));
    };
    let said = line.split_once(".elf: ").map_or(line, |(_, said)| said);
    Ok(said.trim())
}

/// The copy of the source, which goes when this does.
struct Scratch {
    dir: PathBuf,
}

impl Scratch {
    /// Copies `COPIED` from `root` to a fresh directory under the system's
    /// temporary one, named for this process.
    fn of(root: &Path) -> Result<Self, String> {
        let dir = env::temp_dir().join(format!("ts26-breakages-{}", process::id()));
        if dir.exists() {
            fs::remove_dir_all(&dir).map_err(at(&dir))?;
        }
        // Made before anything's copied, so a failed copy is cleared up too.
        let copy = Self { dir };
        for entry in COPIED {
            copy_tree(&root.join(entry), &copy.dir.join(entry))?;
        }
        Ok(copy)
    }

    /// Runs `cargo xtask hex` in the copy. Gives back whether it passed, and
    /// everything it printed.
    fn hex(&self) -> Result<(bool, String), String> {
        let cargo = env::var("CARGO").unwrap_or_else(|_| "cargo".into());
        let output = Command::new(&cargo)
            .current_dir(&self.dir)
            .args(["xtask", "hex"])
            // The copy's xtask looks in the copy's own `target`.
            .env_remove("CARGO_TARGET_DIR")
            .output()
            .map_err(|e| format!("couldn't run {cargo}: {e}"))?;
        let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
        text.push_str(&String::from_utf8_lossy(&output.stderr));
        Ok((output.status.success(), text))
    }

    /// Breaks the copy as `breakage` says, having put back whatever the last
    /// one broke, and checks `hex` refuses it for the reason it gives.
    fn refuses(&self, root: &Path, breakage: &Breakage) -> Result<String, String> {
        for other in BREAKAGES {
            let file = Path::new(other.file);
            fs::copy(root.join(file), self.dir.join(file)).map_err(at(file))?;
        }
        let path = self.dir.join(breakage.file);
        let source = fs::read_to_string(&path).map_err(at(&path))?;
        let broken = substitute(&source, breakage.find, breakage.replace)
            .map_err(|e| format!("{}: {e}", breakage.file))?;
        fs::write(&path, broken).map_err(at(&path))?;

        let (passed, output) = self.hex()?;
        if passed {
            return Err("`cargo xtask hex` passed it".into());
        }
        refused(&output, breakage.stage, breakage.expect).map(str::to_string)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        // Nothing to be done if it won't go, and the run's verdict stands.
        _ = fs::remove_dir_all(&self.dir);
    }
}

/// Copies a file, or a directory and all that's in it but `SKIPPED`.
fn copy_tree(from: &Path, to: &Path) -> Result<(), String> {
    if !from.is_dir() {
        if let Some(parent) = to.parent() {
            fs::create_dir_all(parent).map_err(at(parent))?;
        }
        return fs::copy(from, to).map(drop).map_err(at(from));
    }
    fs::create_dir_all(to).map_err(at(to))?;
    for entry in fs::read_dir(from).map_err(at(from))? {
        let name = entry.map_err(at(from))?.file_name();
        if SKIPPED.iter().any(|skipped| name == *skipped) {
            continue;
        }
        copy_tree(&from.join(&name), &to.join(&name))?;
    }
    Ok(())
}

/// Prefixes an error with the file it came from.
fn at<E: std::fmt::Display>(path: &Path) -> impl Fn(E) -> String + '_ {
    move |e| format!("{}: {e}", path.display())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The checkout these tests were built from.
    fn root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .to_path_buf()
    }

    #[test]
    fn a_substitution_wants_its_text_there_exactly_once() {
        assert_eq!(substitute("a b c", "b", "x").unwrap(), "a x c");
        assert_eq!(substitute("a b c", " b", "").unwrap(), "a c");
        let err = substitute("a b c", "d", "x").unwrap_err();
        assert!(err.contains("there 0 times"), "{err}");
        let err = substitute("a b b", "b", "x").unwrap_err();
        assert!(err.contains("there 2 times"), "{err}");
    }

    #[test]
    fn a_substitution_has_to_change_something() {
        assert!(substitute("a b c", "b", "b").is_err());
        assert!(substitute("a b c", "", "x").is_err());
    }

    /// The slow part isn't run here, so this is what catches a table gone
    /// stale: a file that's moved, or a line a refactor reworded.
    #[test]
    fn every_breakage_finds_its_text_exactly_once_in_the_source_as_it_is() {
        for breakage in BREAKAGES {
            let path = root().join(breakage.file);
            let source = fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("{}: {}: {e}", breakage.name, breakage.file));
            let broken = substitute(&source, breakage.find, breakage.replace)
                .unwrap_or_else(|e| panic!("{}: {}: {e}", breakage.name, breakage.file));
            assert_ne!(broken, source, "{}", breakage.name);
            assert!(!breakage.expect.is_empty(), "{}", breakage.name);
        }
    }

    #[test]
    fn every_breakage_has_its_own_name_and_breaks_a_file_thats_copied() {
        for (i, breakage) in BREAKAGES.iter().enumerate() {
            let twice = BREAKAGES[..i].iter().any(|b| b.name == breakage.name);
            assert!(!twice, "two called {}", breakage.name);
            let copied = COPIED
                .iter()
                .any(|entry| Path::new(breakage.file).starts_with(entry));
            assert!(copied, "{} isn't copied", breakage.file);
        }
    }

    #[test]
    fn every_copied_entry_is_there_to_copy() {
        for entry in COPIED {
            assert!(root().join(entry).exists(), "{entry}");
        }
    }

    #[test]
    fn breakages_are_chosen_by_name_and_an_unknown_name_is_refused() {
        assert_eq!(choose(&[]).unwrap().len(), BREAKAGES.len());
        let chosen = choose(&["wide-multiply", "pb2-pull-up"]).unwrap();
        let names: Vec<&str> = chosen.iter().map(|b| b.name).collect();
        assert_eq!(names, ["pb2-pull-up", "wide-multiply"]);
        let err = choose(&["pb2-pull-up", "pb3-pull-up"]).err().unwrap();
        assert!(err.contains("no breakage called `pb3-pull-up`"), "{err}");
    }

    const SIMULATOR: &str = "\
   Compiling ts26 v0.0.0 (/tmp/copy)
    Finished `release` profile [optimized + debuginfo] target(s) in 4.05s
error: /tmp/copy/target/avr-none/release/ts26.elf: simulator: startup: 0x0a1c: gives PB2 a pull-up
";
    const LAYOUT: &str = "\
    Finished `release` profile [optimized + debuginfo] target(s) in 4.05s
error: /tmp/copy/target/avr-none/release/ts26.elf: the CCP unlock at 0x0010 isn't followed by `st Z`
";
    const LINKER: &str = "\
error: linking with `rust-lld` failed: exit status: 1
  = note: rust-lld: error: undefined symbol: __mulsi3
error: could not compile `ts26` (bin \"ts26\") due to 1 previous error
error: cargo failed (exit status: 101)
";
    const COMPILER: &str = "\
error[E0425]: cannot find value `x` in this scope
error: could not compile `ts26` (bin \"ts26\") due to 1 previous error
error: cargo failed (exit status: 101)
";

    #[test]
    fn the_verdict_is_xtasks_last_error_and_says_how_far_hex_got() {
        let stage = |output| verdict(output).map(|(stage, _)| stage);
        assert_eq!(stage(SIMULATOR), Some(Stage::Simulator));
        assert_eq!(stage(LAYOUT), Some(Stage::Layout));
        assert_eq!(stage(LINKER), Some(Stage::Build));
        assert_eq!(stage(COMPILER), Some(Stage::Build));
        assert_eq!(stage("wrote target/wurkkos-ts26-0721.hex\n"), None);
        let (_, line) = verdict(LINKER).unwrap();
        assert_eq!(line, "cargo failed (exit status: 101)");
    }

    #[test]
    fn a_refusal_counts_only_at_the_stage_and_with_the_text_expected() {
        let line = refused(SIMULATOR, Stage::Simulator, "gives PB2 a pull-up").unwrap();
        assert_eq!(line, "simulator: startup: 0x0a1c: gives PB2 a pull-up");
        let line = refused(LAYOUT, Stage::Layout, "isn't followed by `st Z`").unwrap();
        assert_eq!(line, "the CCP unlock at 0x0010 isn't followed by `st Z`");
        let line = refused(LINKER, Stage::Build, "undefined symbol: __mulsi3").unwrap();
        assert_eq!(line, "= note: rust-lld: error: undefined symbol: __mulsi3");

        // Refused, but for something else.
        let err = refused(SIMULATOR, Stage::Simulator, "without wdr").unwrap_err();
        assert!(err.contains("not with `without wdr`"), "{err}");
        // A breakage that doesn't compile hasn't been refused by anything.
        let err = refused(COMPILER, Stage::Build, "undefined symbol: __mulsi3").unwrap_err();
        assert!(err.contains("not with"), "{err}");
        let err = refused(COMPILER, Stage::Simulator, "x").unwrap_err();
        assert!(err.contains("stopped at Build, not Simulator"), "{err}");
        let err = refused(LAYOUT, Stage::Simulator, "st Z").unwrap_err();
        assert!(err.contains("stopped at Layout, not Simulator"), "{err}");
        let err = refused("", Stage::Build, "x").unwrap_err();
        assert!(err.contains("without saying why"), "{err}");
    }

    #[test]
    fn a_copy_leaves_out_target_and_git_and_goes_when_its_dropped() {
        let from = env::temp_dir().join(format!("ts26-breakages-test-{}", process::id()));
        let to = from.with_extension("copy");
        for dir in ["src/avr", "target", ".git", "src/target"] {
            fs::create_dir_all(from.join(dir)).unwrap();
            fs::write(from.join(dir).join("hw.rs"), "contents").unwrap();
        }
        let copy = Scratch { dir: to.clone() };
        copy_tree(&from, &to).unwrap();
        copy_tree(&from.join("src/avr/hw.rs"), &to.join("docs/hw.rs")).unwrap();
        let copied = fs::read_to_string(to.join("src/avr/hw.rs")).unwrap();
        assert_eq!(copied, "contents");
        assert!(to.join("docs/hw.rs").exists());
        for gone in ["target", ".git", "src/target"] {
            assert!(!to.join(gone).exists(), "{gone}");
        }
        drop(copy);
        assert!(!to.exists());
        fs::remove_dir_all(&from).unwrap();
    }
}
