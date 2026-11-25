//! A refusal shows one alarm rings for one mistake, not for every mistake of
//! that kind.

use std::fs::{self, File};
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;

use crate::{TempDir, at, checked, firmware_build, guards_checked, root};

const COPIED: [&str; 10] = [
    ".cargo",
    "Cargo.lock",
    "Cargo.toml",
    "build.rs",
    "examples",
    "link.x",
    "rust-toolchain.toml",
    "src",
    "vendor",
    "xtask",
];

const SKIPPED: [&str; 2] = ["target", ".git"];

struct Mistake {
    name: &'static str,
    file: &'static str,
    /// Text that's in `file` exactly once.
    find: &'static str,
    replace: &'static str,
    expect: &'static str,
}

const HW: &str = "src/avr/hw.rs";
const MAIN: &str = "src/main.rs";
const RT: &str = "src/avr/rt.rs";

const MISTAKES: &[Mistake] = &[
    Mistake {
        name: "enable-before-duty",
        file: HW,
        find: "    let stopped = read(reg::TCA0_INTCTRL) == 0;\n",
        replace: "    let stopped = read(reg::TCA0_INTCTRL) == 0;\n    \
                  write(reg::VPORTB_OUT, read(reg::VPORTB_OUT) | 1 << ENABLE);\n",
        expect: "raises PB2 with the duty at 0",
    },
    Mistake {
        name: "duty-before-enable",
        file: HW,
        find: "    if !output.enable {\n        \
               write(reg::VPORTB_OUT, read(reg::VPORTB_OUT) & !(1 << ENABLE));\n    }\n",
        replace: "",
        expect: "writes TCA0.CMP0BUF from the main loop with PB2 high",
    },
    Mistake {
        name: "pb0-left-to-tca0",
        file: HW,
        find: "            write(reg::TCA0_CTRLB, CTRLB_OFF);\n",
        replace: "",
        expect: "powers down with TCA0.CTRLB.CMP0EN set",
    },
    // Timing-dependent. Refused only because `hw::halt` powers down within a
    // cycle of turning off, and a BOTTOM in between would hide it.
    Mistake {
        name: "no-direct-compare-write",
        file: HW,
        find: "            write16(reg::TCA0_CMP0, 0);\n",
        replace: "",
        expect: "powers down with CMP0 at",
    },
    Mistake {
        name: "level-change-with-interrupts-on",
        file: HW,
        find: "    unsafe { asm!(\"cli\", options(nostack)) };\n    f();\n",
        replace: "    f();\n",
        expect: "so the handler can catch the delta-sigma half changed",
    },
    Mistake {
        name: "sum-kept-at-turn-off",
        file: HW,
        find: "            write(DSM_SUM, 0);\n",
        replace: "",
        expect: "with the delta-sigma's sum at",
    },
    Mistake {
        name: "stale-overflow-flag",
        file: HW,
        find: "            write(reg::TCA0_INTFLAGS, regs::TCA_OVF);\n",
        replace: "",
        expect: "with INTFLAGS.OVF still up from before",
    },
    Mistake {
        name: "halt-leaves-the-switch-sensing",
        file: HW,
        find: "    write(SWITCH_CTRL, SENSE_OFF);\n    prepare_sleep(true);\n",
        replace: "    prepare_sleep(true);\n",
        expect: "sleeps inside the handler for interrupt 13, so no other can wake it",
    },
    Mistake {
        name: "halt-without-cli",
        file: HW,
        find: "    unsafe { asm!(\"cli\", options(nostack)) };\n    set_output(Output::OFF);\n",
        replace: "    set_output(Output::OFF);\n",
        expect: "stops for good with SREG.I still set",
    },
    Mistake {
        name: "pb2-raised-while-charging",
        file: HW,
        find: "    if output.enable {\n        \
               write(reg::VPORTB_OUT, read(reg::VPORTB_OUT) | 1 << ENABLE);\n    }\n",
        replace: "    write(reg::VPORTB_OUT, read(reg::VPORTB_OUT) | 1 << ENABLE);\n",
        expect: "PB2 is true, but the library says Output { enable: false",
    },
    Mistake {
        name: "pb2-pull-up",
        file: HW,
        find: "        if PORTB_OUTPUTS & 1 << pin == 0 {\n",
        replace: "        if 1 << PWM & 1 << pin == 0 {\n",
        expect: "gives PB2 a pull-up",
    },
    Mistake {
        name: "high-byte-first",
        file: HW,
        find: "    write(addr, lo);\n    write(addr + 1, hi);\n",
        replace: "    write(addr + 1, hi);\n    write(addr, lo);\n",
        expect: "writes the high byte of TCA0.PER without its low byte",
    },
    Mistake {
        name: "watchdog-never-fed",
        file: MAIN,
        find: "            Some(Step::Feed) => hw::feed_watchdog(),\n",
        replace: "            Some(Step::Feed) => {}\n",
        expect: "without wdr",
    },
    Mistake {
        name: "watchdog-fed-every-tick",
        file: MAIN,
        find: "            None => {}\n",
        replace: "            None => hw::feed_watchdog(),\n",
        expect: "closed window",
    },
    Mistake {
        name: "power-down-straight-after-the-window",
        file: MAIN,
        find: "hw::wait_for_tick(torch.resting() && pace.powers_down())",
        replace: "hw::wait_for_tick(torch.resting())",
        expect: "the library says idle, but the firmware powered down",
    },
    Mistake {
        name: "window-left-on-at-rest",
        file: HW,
        find: "    set_watchdog(regs::WDT_PERIOD_8KCLK);\n",
        replace: "",
        expect: "slows the PIT with the watchdog's window still on",
    },
    Mistake {
        name: "falling-edge-at-rest",
        file: HW,
        find: "const SENSE_ON: u8 = regs::PORT_PULLUPEN | regs::PORT_ISC_BOTHEDGES;\n",
        replace: "const SENSE_ON: u8 = regs::PORT_PULLUPEN | regs::PORT_ISC_FALLING;\n",
        expect: "the switch changed at rest, but the firmware slept through it",
    },
    Mistake {
        name: "switch-flag-left-up",
        file: HW,
        find: "    write(SWITCH_CTRL, SENSE_OFF);\n    write(reg::PORTA_INTFLAGS, 1 << SWITCH);\n",
        replace: "    write(SWITCH_CTRL, SENSE_OFF);\n",
        expect: "the handler for PORTA_PORT (vector 3) returns with its flag still up",
    },
    Mistake {
        name: "pit-left-at-1hz-on-a-press",
        file: MAIN,
        find: "            pace.woken()\n",
        replace: "            pace.woken().filter(|_| false)\n",
        expect: "the firmware woke on a press at rest, but left the PIT at 1 Hz",
    },
    // The firmware's cell check at rest then comes 64 times later than the
    // library's.
    Mistake {
        name: "rest-tick-worth-one",
        file: MAIN,
        find: "hw::adc_result(), pace.ticks());\n",
        replace: "hw::adc_result(), 1);\n",
        expect: "the library ticks at 64 Hz, but the PIT at 1 Hz",
    },
    Mistake {
        name: "adc-on-in-power-down",
        file: HW,
        find: "        write(reg::ADC0_CTRLA, 0);\n        write(reg::VREF_CTRLB, 0);\n",
        replace: "        write(reg::VREF_CTRLB, 0);\n",
        expect: "powers down with ADC0 still enabled",
    },
    // Still turns TCA0's interrupt off, so the `sleep` is one the chip would
    // take, with PB2 high.
    Mistake {
        name: "halt-leaves-the-light-on",
        file: HW,
        find: "    set_output(Output::OFF);\n    write(reg::RTC_PITINTCTRL, 0);\n",
        replace: "    write(reg::TCA0_INTCTRL, 0);\n    write(reg::RTC_PITINTCTRL, 0);\n",
        expect: "stopped for good with PB2 high",
    },
    Mistake {
        name: "dir-before-out",
        file: HW,
        find: "    write(reg::VPORTB_OUT, 0);\n    write(reg::VPORTB_DIR, PORTB_OUTPUTS);\n",
        replace: "    write(reg::VPORTB_DIR, PORTB_OUTPUTS);\n    write(reg::VPORTB_OUT, 0);\n",
        expect: "starting with PB2 high: the first register it writes leaves PB2 set",
    },
    Mistake {
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
        expect: "start-up order: turns the watchdog on before clearing the reset flags",
    },
    Mistake {
        name: "handler-races-temp",
        file: HW,
        find: "            write16(reg::TCA0_CMP0BUF, u16::from(duty));\n            \
               write(reg::TCA0_CTRLB, CTRLB_ON);\n            \
               write(reg::TCA0_INTFLAGS, regs::TCA_OVF);\n            \
               write(reg::TCA0_INTCTRL, regs::TCA_OVF);\n        }\n    });\n",
        replace: "            write(reg::TCA0_CTRLB, CTRLB_ON);\n            \
                  write(reg::TCA0_INTFLAGS, regs::TCA_OVF);\n            \
                  write(reg::TCA0_INTCTRL, regs::TCA_OVF);\n        }\n    });\n    \
                  if stopped {\n        \
                  write16(reg::TCA0_CMP0BUF, u16::from(duty));\n    }\n",
        expect: "share TCA0's TEMP byte",
    },
    Mistake {
        name: "no-tick-deadline",
        file: HW,
        find: "            cycles = left;\n",
        replace: "            cycles = left.max(1);\n",
        expect: "past the deadline",
    },
    Mistake {
        name: "tick-deadline-only-when-lit",
        file: HW,
        find: "        if read(reg::TCA0_INTCTRL) != 0 {\n            let Some(left)",
        replace: "        if read(reg::VPORTB_OUT) & 1 << ENABLE != 0 {\n            let Some(left)",
        expect: "stopped mid-press: the PWM's still running",
    },
    Mistake {
        name: "unlock-apart-from-its-write",
        file: HW,
        find: "            \"out {ccp}, {key}\",\n",
        replace: "            \"out {ccp}, {key}\",\n            \"nop\",\n",
        expect: "isn't followed by `st Z`",
    },
    Mistake {
        name: "handler-clobbers-a-register",
        file: HW,
        find: "    write(reg::TCA0_INTFLAGS, regs::TCA_OVF);\n    let sum =",
        replace: "    write(reg::TCA0_INTFLAGS, regs::TCA_OVF);\n    \
                  unsafe { asm!(\"inc r2\", options(nostack)) };\n    let sum =",
        expect: "the handler for TCA0_OVF (vector 8): returns with r2",
    },
    // The panic check goes by symbol names, so a new nightly can quietly
    // rename them past it.
    Mistake {
        name: "bounds-check",
        file: MAIN,
        find: "            torch.tick(hw::switch_pressed(), hw::adc_result(), pace.ticks());\n",
        replace: "            let reading = hw::adc_result();\n            \
                  let pressed = [hw::switch_pressed(); 4][usize::from(reading.unwrap_or(0))];\n            \
                  torch.tick(pressed, reading, pace.ticks());\n",
        expect: "panic_bounds_check",
    },
    // Dividing a `u16` by ten compiles to a 32-bit multiply.
    Mistake {
        name: "wide-multiply",
        file: HW,
        find: "    Some(u16::from_le_bytes([lo, hi]))\n",
        replace: "    Some(u16::from_le_bytes([lo, hi]) / 10)\n",
        expect: "undefined symbol: __mulsi3",
    },
    Mistake {
        name: "pass-too-long-for-the-watchdogs-sums",
        file: MAIN,
        find: "            torch.tick(hw::switch_pressed(), hw::adc_result(), pace.ticks());\n",
        replace: "            for _ in 0..2000_u16 {\n                \
                  hw::switch_pressed();\n            }\n            \
                  torch.tick(hw::switch_pressed(), hw::adc_result(), pace.ticks());\n",
        expect: "a pass took",
    },
    Mistake {
        name: "enable-held-after-the-self-test",
        file: MAIN,
        find: "    hw::hold_enable_low();\n\n    \
               let faults = Faults {\n        \
               hang: selftest::hung(hw::take_reset_flags()),\n        \
               flash: !hw::flash_ok(),\n        \
               fuses: !hw::fuses().ok(),\n    };\n",
        replace: "    let faults = Faults {\n        \
                  hang: selftest::hung(hw::take_reset_flags()),\n        \
                  flash: !hw::flash_ok(),\n        \
                  fuses: !hw::fuses().ok(),\n    };\n\n    \
                  hw::hold_enable_low();\n",
        expect: "leaves PB2 set",
    },
    Mistake {
        name: "clock-divided-by-four",
        file: HW,
        find: "        regs::CLKCTRL_PDIV_2X | regs::CLKCTRL_PEN,\n",
        replace: "        regs::CLKCTRL_PDIV_4X | regs::CLKCTRL_PEN,\n",
        expect: "the firmware runs at 10 MHz",
    },
    Mistake {
        name: "battery-pin-pull-up",
        file: HW,
        find: "            BATTERY => regs::PORT_ISC_INPUT_DISABLE,\n",
        replace: "            BATTERY => regs::PORT_PULLUPEN,\n",
        expect: "battery divider",
    },
    Mistake {
        name: "updi-pin-written",
        file: HW,
        find: "            UPDI => continue,\n",
        replace: "",
        expect: "PA0 is UPDI, and the firmware leaves its settings alone",
    },
    Mistake {
        name: "switch-input-buffer-off",
        file: HW,
        find: "const SENSE_OFF: u8 = regs::PORT_PULLUPEN | regs::PORT_ISC_INTDISABLE;\n",
        replace: "const SENSE_OFF: u8 = regs::PORT_PULLUPEN | regs::PORT_ISC_INPUT_DISABLE;\n",
        expect: "reads PA5 with its input buffer off",
    },
    Mistake {
        name: "period-after-the-timer-starts",
        file: HW,
        find: "    write16(reg::TCA0_PER, PWM_TOP);\n    \
               write16(reg::TCA0_CMP0BUF, 0);\n    \
               write(reg::TCA0_CTRLB, CTRLB_OFF);\n    \
               write(reg::TCA0_INTCTRL, 0);\n    \
               write(reg::TCA0_CTRLA, regs::TCA_CLKSEL_DIV1 | regs::TCA_ENABLE);\n",
        replace: "    write16(reg::TCA0_CMP0BUF, 0);\n    \
                  write(reg::TCA0_CTRLB, CTRLB_OFF);\n    \
                  write(reg::TCA0_INTCTRL, 0);\n    \
                  write(reg::TCA0_CTRLA, regs::TCA_CLKSEL_DIV1 | regs::TCA_ENABLE);\n    \
                  write16(reg::TCA0_PER, PWM_TOP);\n",
        expect: "writes TCA0.PER with the timer running",
    },
    Mistake {
        name: "adc-duty-left-at-reset",
        file: HW,
        find: "    write(reg::ADC0_CALIB, regs::ADC_DUTYCYC_DUTY25);\n",
        replace: "",
        expect: "without ADC0.CALIB at 25% duty",
    },
    Mistake {
        name: "adc-clock-too-fast",
        file: HW,
        find: "regs::ADC_REFSEL_INTREF | regs::ADC_PRESC_DIV16,\n",
        replace: "regs::ADC_REFSEL_INTREF | regs::ADC_PRESC_DIV2,\n",
        expect: "outside 200 kHz to 1.5 MHz",
    },
    Mistake {
        name: "no-sampling-capacitor",
        file: HW,
        find: "        regs::ADC_SAMPCAP | regs::ADC_REFSEL_INTREF",
        replace: "        regs::ADC_REFSEL_INTREF",
        expect: "it needs SAMPCAP",
    },
    Mistake {
        name: "no-initial-delay",
        file: HW,
        find: "    write(reg::ADC0_CTRLD, regs::ADC_INITDLY_DLY32);\n",
        replace: "",
        expect: "reads the temperature sensor with INITDLY 0",
    },
    Mistake {
        name: "sample-too-short",
        file: HW,
        find: "    write(reg::ADC0_SAMPCTRL, regs::ADC_SAMPLEN_32US);\n",
        replace: "",
        expect: "reads the temperature sensor with INITDLY",
    },
    Mistake {
        name: "pit-written-mid-sync",
        file: HW,
        find: "    while read(reg::RTC_PITSTATUS) & regs::RTC_CTRLBUSY != 0 {}\n    \
               write(reg::RTC_PITCTRLA",
        replace: "    write(reg::RTC_PITCTRLA",
        expect: "CTRLBUSY is set",
    },
    Mistake {
        name: "pit-sync-not-waited-out",
        file: HW,
        find: "regs::RTC_PITEN);\n    \
               while read(reg::RTC_PITSTATUS) & regs::RTC_CTRLBUSY != 0 {}\n",
        replace: "regs::RTC_PITEN);\n",
        expect: "powers down with RTC.PITSTATUS.CTRLBUSY still set",
    },
    Mistake {
        name: "pit-turned-off",
        file: HW,
        find: "period | regs::RTC_PITEN",
        replace: "period",
        expect: "sleeps with no interrupt enabled that could wake it",
    },
    Mistake {
        name: "sleep-not-enabled",
        file: HW,
        find: "mode | regs::SLPCTRL_SEN",
        replace: "mode",
        expect: "sleeps with SLPCTRL.SEN clear",
    },
    Mistake {
        name: "sleep-apart-from-sei",
        file: HW,
        find: "asm!(\"sei\", \"sleep\", options(nostack))",
        replace: "asm!(\"sei\", \"nop\", \"sleep\", options(nostack))",
        expect: "doesn't follow `sei`",
    },
    Mistake {
        name: "pit-slowed-before-the-watchdog",
        file: HW,
        find: "    set_watchdog(regs::WDT_PERIOD_8KCLK);\n    set_pit(regs::RTC_PERIOD_CYC32768);\n",
        replace: "    set_pit(regs::RTC_PERIOD_CYC32768);\n    set_watchdog(regs::WDT_PERIOD_8KCLK);\n",
        expect: "slows the PIT with the watchdog's window still on",
    },
    Mistake {
        name: "late-rest-tick-kept",
        file: HW,
        find: "    set_pit(regs::RTC_PERIOD_CYC512);\n    write(reg::GPIO_GPIOR0, 0);\n",
        replace: "    set_pit(regs::RTC_PERIOD_CYC512);\n",
        expect: "with a 1 Hz tick landing as a press speeds the PIT up",
    },
    Mistake {
        name: "window-back-without-its-wdr",
        file: HW,
        find: "    start_watchdog();\n    feed_watchdog();\n",
        replace: "    start_watchdog();\n",
        expect: "without wdr",
    },
    Mistake {
        name: "watchdog-sync-not-waited-out",
        file: HW,
        find: "    protected_write(reg::WDT_CTRLA, ctrla);\n    \
               while read(reg::WDT_STATUS) & regs::WDT_SYNCBUSY != 0 {}\n",
        replace: "    protected_write(reg::WDT_CTRLA, ctrla);\n",
        expect: "wdr before the WDT.CTRLA write has synced",
    },
    Mistake {
        name: "crc-status-read-too-soon",
        file: HW,
        find: "    // SAFETY: four `nop`s, which touch nothing.\n    \
               unsafe {\n        asm!(\n            \
               \"nop\",\n            \"nop\",\n            \"nop\",\n            \"nop\",\n            \
               options(nostack, preserves_flags)\n        );\n    }\n",
        replace: "",
        expect: "but the library says Blue",
    },
    Mistake {
        name: "adc-reference-not-forced",
        file: HW,
        find: "    write(reg::VREF_CTRLB, regs::VREF_ADC0REFEN);\n",
        replace: "",
        expect: "ADC0REFEN clear",
    },
    Mistake {
        name: "adc-result-high-byte-first",
        file: HW,
        find: "    let lo = read(reg::ADC0_RES);\n    let hi = read(reg::ADC0_RES + 1);\n",
        replace: "    let hi = read(reg::ADC0_RES + 1);\n    let lo = read(reg::ADC0_RES);\n",
        expect: "without its low byte",
    },
    Mistake {
        name: "halt-leaves-the-tick-on",
        file: HW,
        find: "    set_output(Output::OFF);\n    write(reg::RTC_PITINTCTRL, 0);\n",
        replace: "    set_output(Output::OFF);\n",
        expect: "sleeps inside the handler for interrupt 13, so no other can wake it",
    },
    Mistake {
        name: "unhandled-interrupt-returns",
        file: RT,
        find: "    \"  jmp {halt}\",\n",
        replace: "    \"  reti\",\n    \"  jmp {halt}\",\n",
        expect: "doesn't `jmp` after clearing r1",
    },
    Mistake {
        name: "unhandled-interrupt-keeps-r1",
        file: RT,
        find: "    \"  eor r1, r1\",\n",
        replace: "",
        expect: "doesn't start with `eor r1, r1`",
    },
];

/// The firmware never asks for these, so only examples/guards.rs reaches them,
/// and only its checks are run on them.
const GUARDS_MISTAKES: &[Mistake] = &[
    Mistake {
        name: "power-down-with-the-light-on",
        file: HW,
        find: "    let mode = if rest && pb2 == 0 && read(reg::TCA0_INTCTRL) == 0 {\n",
        replace: "    let mode = if rest {\n",
        expect: "powers down with PB2 high",
    },
    // Charges the filter with power-down asked for.
    Mistake {
        name: "power-down-while-charging",
        file: HW,
        find: "    let mode = if rest && pb2 == 0 && read(reg::TCA0_INTCTRL) == 0 {\n",
        replace: "    let mode = if rest && pb2 == 0 {\n",
        expect: "powers down with the delta-sigma running",
    },
];

type Check = fn(&Path) -> Result<(), String>;

fn hex_checked(copy: &Path) -> Result<(), String> {
    checked(copy).map(drop)
}

fn mistakes() -> Vec<(&'static Mistake, Check)> {
    let whole = MISTAKES
        .iter()
        .map(|mistake| (mistake, hex_checked as Check));
    let guards = GUARDS_MISTAKES
        .iter()
        .map(|mistake| (mistake, guards_checked as Check));
    whole.chain(guards).collect()
}

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

fn copy_tree(from: &Path, to: &Path) -> Result<(), String> {
    if !from.is_dir() {
        if let Some(parent) = to.parent() {
            fs::create_dir_all(parent).map_err(at(parent))?;
        }
        fs::copy(from, to).map_err(at(from))?;
        // Linux's copy stamps the file with now, and cargo tells a fresh
        // build from a stale one by which file is the newer.
        let modified = fs::metadata(from).and_then(|from| from.modified());
        let copy = File::options().write(true).open(to);
        return modified
            .and_then(|modified| copy?.set_modified(modified))
            .map_err(at(to));
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

/// What `cargo xtask hex` does up to writing the hex, or the share of it
/// that's `check`, with cargo's own refusal kept, since a link error is one of
/// the reasons.
fn refused(copy: &Path, check: Check) -> Result<(), String> {
    let built = firmware_build(copy).output().map_err(|e| e.to_string())?;
    if !built.status.success() {
        return Err(String::from_utf8_lossy(&built.stderr).into_owned());
    }
    check(copy)
}

#[test]
fn a_substitution_wants_its_text_there_exactly_once_and_has_to_change_it() {
    assert_eq!(substitute("a b c", "b", "x").unwrap(), "a x c");
    assert_eq!(substitute("a b c", " b", "").unwrap(), "a c");
    for (source, find, replace, why) in [
        ("a b c", "d", "x", "there 0 times"),
        ("a b b", "b", "x", "there 2 times"),
        ("a b c", "b", "b", "changes nothing"),
        ("a b c", "", "x", "changes nothing"),
    ] {
        let err = substitute(source, find, replace).unwrap_err();
        assert!(err.contains(why), "{err}");
    }
}

#[test]
fn every_mistake_has_its_own_name_and_finds_its_text_exactly_once() {
    let mistakes = mistakes();
    for (i, (mistake, _)) in mistakes.iter().enumerate() {
        let twice = mistakes[..i].iter().any(|(m, _)| m.name == mistake.name);
        assert!(!twice, "two called {}", mistake.name);
        assert!(!mistake.expect.is_empty(), "{}", mistake.name);
        let copied = COPIED
            .iter()
            .any(|entry| Path::new(mistake.file).starts_with(entry));
        assert!(copied, "{} isn't copied", mistake.file);
        let path = root().join(mistake.file);
        let source = fs::read_to_string(&path).unwrap();
        substitute(&source, mistake.find, mistake.replace)
            .unwrap_or_else(|e| panic!("{}: {}: {e}", mistake.name, mistake.file));
    }
}

// The simulator here is built from the unbroken library, so a broken one may
// be refused only for disagreeing with it.
#[test]
fn every_mistake_is_outside_the_library_whose_own_tests_catch_its_mistakes() {
    for (mistake, _) in mistakes() {
        let firmware = mistake.file == MAIN || mistake.file.starts_with("src/avr/");
        assert!(firmware, "{}: {}", mistake.name, mistake.file);
    }
}

/// Where cargo keeps the `core` and `compiler_builtins` it built for the
/// firmware. A copy that starts with them doesn't build its own, and one that
/// can't only takes longer.
const BUILT: [&str; 3] = [
    "target/avr-none/release/build/core",
    "target/avr-none/release/build/compiler_builtins",
    "target/release/build/compiler_builtins",
];

fn copy_of_the_tree(name: &str) -> TempDir {
    let dir = TempDir::new(name).unwrap();
    for entry in COPIED {
        copy_tree(&root().join(entry), &dir.path().join(entry)).unwrap();
    }
    for entry in BUILT {
        _ = copy_tree(&root().join(entry), &dir.path().join(entry));
    }
    // A mistake that leaves dead code behind would stop at the compiler,
    // before the check it names.
    let manifest = dir.path().join("Cargo.toml");
    let denied = fs::read_to_string(&manifest).unwrap();
    let warned = substitute(&denied, "warnings = \"deny\"\n", "").unwrap();
    fs::write(&manifest, warned).unwrap();
    dir
}

/// Gives back what wasn't refused as expected, each with its place in `mistakes`.
fn try_mistakes(
    copy: &Path,
    mistakes: &[(&Mistake, Check)],
    next: &AtomicUsize,
) -> Vec<(usize, String)> {
    let mut missed = Vec::new();
    loop {
        let i = next.fetch_add(1, Ordering::Relaxed);
        let Some(&(mistake, check)) = mistakes.get(i) else {
            return missed;
        };
        let path = copy.join(mistake.file);
        let source = fs::read_to_string(&path).unwrap();
        let broken = substitute(&source, mistake.find, mistake.replace).unwrap();
        fs::write(&path, broken).unwrap();
        let refusal = refused(copy, check);
        // Written rather than copied back, since a copy can keep the old
        // modified time, and cargo would then keep the broken build.
        fs::write(&path, source).unwrap();
        match refusal {
            Err(e) if e.contains(mistake.expect) => {}
            Err(e) => missed.push((
                i,
                format!(
                    "{}: refused, but not with `{}`:\n{e}",
                    mistake.name, mistake.expect
                ),
            )),
            Ok(()) => missed.push((i, format!("{}: nothing refused it", mistake.name))),
        }
    }
}

/// As many as CI's runner has cores.
const COPIES: usize = 4;

#[test]
#[ignore = "builds the firmware once a mistake, which takes about ten seconds"]
fn the_firmware_is_refused_with_any_one_mistake_in_it() {
    // Leaves `BUILT` built, and nothing writes it again while it's copied.
    let built = firmware_build(&root()).output().unwrap();
    let stderr = String::from_utf8_lossy(&built.stderr);
    assert!(built.status.success(), "{stderr}");

    let (mistakes, next) = (mistakes(), AtomicUsize::new(0));
    let mut missed: Vec<(usize, String)> = thread::scope(|scope| {
        let copies: Vec<_> = (0..COPIES)
            .map(|n| {
                let (mistakes, next) = (&mistakes, &next);
                scope.spawn(move || {
                    let copy = copy_of_the_tree(&format!("mistakes-{n}"));
                    try_mistakes(copy.path(), mistakes, next)
                })
            })
            .collect();
        copies
            .into_iter()
            .flat_map(|copy| copy.join().unwrap())
            .collect()
    });
    missed.sort_by_key(|(i, _)| *i);
    let mut missed: Vec<String> = missed.into_iter().map(|(_, why)| why).collect();
    if !missed.is_empty()
        && let Err(e) = refused(copy_of_the_tree("mistakes-none").path(), hex_checked)
    {
        missed.insert(
            0,
            format!(
                "a copy with nothing broken is refused too, so no refusal means anything:\n{e}"
            ),
        );
    }
    assert!(missed.is_empty(), "{}", missed.join("\n\n"));
}
