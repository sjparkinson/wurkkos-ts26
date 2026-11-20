//! A fake ATtiny1616 around `avr.rs`'s CPU, wired up like the TS26's driver,
//! and the checks that run the real firmware on it.
//!
//! This finds mistakes and proves nothing. It is one reading of the datasheet,
//! run against code written from that same reading, and has never been
//! compared with a real chip. It doesn't model resets, so a watchdog bite is a
//! failed run, nor anything analogue. The board's rules have no section
//! number, since they come from what the stock firmware does.
//!
//! Guesses where the datasheet is silent, all unconfirmed.
//!
//! - A TCA0 cycle is 2 × PER clocks, worked back from §20.3.3.4.4's frequency.
//!   Whether OVF and the BUF copies (table 20-6) land on the same clock, and
//!   what a BUF write on that clock does, aren't modelled.
//! - A timer that's just been enabled isn't taken as being at BOTTOM, so the
//!   whole first cycle runs at whatever PER is.
//! - VPORTA.IN follows the pin at once. The real one stops following it in
//!   power-down, and nothing says how soon after a wake it catches up
//!   (§16.3.5).
//! - Power-down wants PB0 back with PORT, since nothing says what a waveform
//!   output holds with TCA0 stopped (§20.3.6), and CMP0 and CMP0BUF at 0,
//!   since registers keep their contents through sleep (§11.2).
//! - `sleep` with SREG.I clear and an interrupt still enabled is an error,
//!   and `hw::halt` wants SREG.I clear too. The only evidence is figure 13-1
//!   having SREG.I gate the wake-up.
//!
//! Time counts instructions, not cycles, so the budgets take every
//! instruction as the slowest, four clocks.

use std::iter;
use std::ops::Range;
use std::thread;

use ts26::TICKS_PER_SECOND;
use ts26::board;
use ts26::ramp::{DSM_BITS, DSM_FLOOR, Output, PWM_TOP};
use ts26::sensors::Channel;
use ts26::testing::{self, Action, EXAMPLES, Example, Rig, Sample, Scene};
use ts26::watchdog::{CLOCK_HZ, FEED_TICKS, Feeder, TICK_DEADLINE_CYCLES};

use crate::avr::{Bus, Cpu, I, VECTOR_WORDS, is_two_words, ldi};
use crate::crc;
use crate::elf::{DATA_SPACE, Elf, SRAM};
use crate::layout::{self, jmp_target, word};
use crate::stamp;
use crate::switching::RandomSwitching;
use crate::updi::map;
use crate::world::{CALIBRATION, FUSE_NAMES, World};

// Only ATDF-generated names, since anything hand-written in src/regs.rs is the
// firmware's choice, and the model mustn't move with the firmware.
use ts26::regs::{
    self, ADC_ENABLE, ADC_MUXPOS_AIN1, ADC_MUXPOS_TEMPSENSE, ADC_PRESC, ADC_REFSEL,
    ADC_REFSEL_INTREF, ADC_RESRDY, ADC_SAMPCAP, ADC_SAMPLEN, ADC_SAMPNUM, ADC_SAMPNUM_ACC4,
    ADC_STCONV, CRCSCAN_BUSY, CRCSCAN_ENABLE, CRCSCAN_OK, PORT_INVEN, PORT_ISC, PORT_PULLUPEN,
    RSTCTRL_PORF, RTC_CTRLBUSY, RTC_PI, RTC_PITEN, SLPCTRL_SEN, TCA_CMP0EN, TCA_OVF,
    VREF_ADC0REFEN, VREF_ADC0REFSEL, WDT_LOCK, WDT_PERIOD, WDT_SYNCBUSY, WDT_WINDOW, addr as reg,
};

const TCA0_WIDE_END: usize = reg::TCA0_CMP2BUF + 2;

const CCP_IOREG: u8 = regs::CPU_CCP_IOREG;
const CLOCK_10MHZ: u8 = regs::CLKCTRL_PDIV_2X | regs::CLKCTRL_PEN;
const ISC_INTDISABLE: u8 = regs::PORT_ISC_INTDISABLE;
const ISC_BOTHEDGES: u8 = regs::PORT_ISC_BOTHEDGES;
const ISC_RISING: u8 = regs::PORT_ISC_RISING;
const ISC_FALLING: u8 = regs::PORT_ISC_FALLING;
const ISC_INPUT_DISABLE: u8 = regs::PORT_ISC_INPUT_DISABLE;
const ISC_LEVEL: u8 = regs::PORT_ISC_LEVEL;
const TCA_RUNNING: u8 = regs::TCA_CLKSEL_DIV1 | regs::TCA_ENABLE;
const TCA_DSBOTTOM: u8 = regs::TCA_WGMODE_DSBOTTOM;
const SLEEP_IDLE: u8 = regs::SLPCTRL_SMODE_IDLE;
const SLEEP_POWER_DOWN: u8 = regs::SLPCTRL_SMODE_PDOWN;

const VECTOR_PORTA: u8 = regs::VECTOR_PORTA_PORT as u8;
const VECTOR_PORTB: u8 = regs::VECTOR_PORTB_PORT as u8;
const VECTOR_PORTC: u8 = regs::VECTOR_PORTC_PORT as u8;
const VECTOR_RTC_PIT: u8 = regs::VECTOR_RTC_PIT as u8;
const VECTOR_TCA0_OVF: u8 = regs::VECTOR_TCA0_OVF as u8;
const VECTORS: usize = regs::VECTORS;

const VREF_1V1: u8 = regs::VREF_ADC0REFSEL_1V1;
/// Anything over 8KCLK is reserved.
const WDT_PERIOD_MAX: u8 = regs::WDT_PERIOD_8KCLK;
const CRCSCAN_FLASH_PRIORITY: u8 = regs::CRCSCAN_SRC_FLASH | regs::CRCSCAN_MODE_PRIORITY;
const ADC_DUTY25: u8 = regs::ADC_DUTYCYC_DUTY25;

const CLK_PER: u32 = 10_000_000;
const _: () = assert!(CLOCK_HZ == CLK_PER);
/// Unmeasured; any non-zero count makes the firmware wait.
const WDT_SYNC_READS: u8 = 2;
/// The PIT divides the same oscillator by 512, and the WDT by 32.
const WDT_CLOCKS_PER_TICK: u32 = 16;
/// Three cycles after ENABLE (§27.3.1), and no instruction is quicker than a
/// cycle.
const CRC_START_INSTRUCTIONS: u64 = 3;
/// Priority mode stalls the CPU, so a real chip may never show BUSY.
const CRC_BUSY_READS: u8 = 2;
/// About 165 ADC clocks for four accumulated, at 16 CPU cycles each.
const CONVERSION_INSTRUCTIONS: u64 = 165 * 16;
/// The datasheet doesn't promise the first conversion after a change of input
/// is wrong, but the stock firmware finds it unstable.
const UNSETTLED: u16 = 0;

const MAPPED: u16 = map::FLASH;
const FLASH_SIZE: usize = map::FLASH_SIZE;

const JUNK: u8 = 0xA5;

const SWITCH: u8 = 1 << board::SWITCH; // PA5, active low
const PWM: u8 = 1 << board::PWM; // PB0, the brightness
const ENABLE: u8 = 1 << board::ENABLE; // PB2, the boost converter
const SWITCH_PIN: usize = board::SWITCH as usize;
const ENABLE_PIN: usize = board::ENABLE as usize;
const UPDI_PIN: usize = board::UPDI as usize;
const BATTERY_PIN: usize = board::BATTERY as usize;
const AUX: u8 = board::AUX_PINS;

const BOOT_BUDGET: u64 = 100_000;
const TICK_CYCLES: u64 = CLK_PER as u64 / TICKS_PER_SECOND as u64;
const _: () = assert!(TICK_CYCLES * TICKS_PER_SECOND as u64 == CLK_PER as u64);
/// No AVRxt instruction takes more than 4 cycles.
pub const TICK_BUDGET: u64 = TICK_CYCLES / 4;
const PWM_CYCLE: u64 = 2 * PWM_TOP as u64;
/// Six in and four for `reti` on a 16 KB part (§13.3.2.3, table 13-1).
const INTERRUPT_CYCLES: u64 = 10;
pub const HANDLER_BUDGET: u64 = (PWM_CYCLE - INTERRUPT_CYCLES) / 4;
/// At the lowest level the delta-sigma only carries into the duty once every
/// 2^DSM_BITS / DSM_FLOOR cycles, so a duty of 0 for a cycle is normal there.
const ZERO_DUTY_CYCLES: u32 = (1 << DSM_BITS) / DSM_FLOOR as u32 + 1;
const DSM_WINDOW: usize = 1 << DSM_BITS;
const DSM_SLACK: u32 = 2;
/// The firmware's layout, not the chip's, so a firmware that moves these has
/// to move them here too.
const DSM_DUTY: usize = reg::GPIO_GPIOR1;
const DSM_FRACTION: usize = reg::GPIO_GPIOR2;
const DSM_SUM: usize = reg::GPIO_GPIOR3;

/// Unmeasured; any non-zero count makes the firmware wait.
const PIT_SYNC_READS: u8 = 2;

#[derive(Clone, Copy, Default)]
pub struct Report {
    pub stack: u16,
    /// From the runs without overflows landed on purpose.
    pub loop_instructions: u64,
    pub off_instructions: u64,
    pub handler_instructions: u64,
    landed: u32,
    last_new: u32,
}

impl Report {
    fn merge(&mut self, run: &Self) {
        self.stack = self.stack.max(run.stack);
        self.loop_instructions = self.loop_instructions.max(run.loop_instructions);
        self.off_instructions = self.off_instructions.max(run.off_instructions);
        self.handler_instructions = self.handler_instructions.max(run.handler_instructions);
        self.landed += run.landed;
    }

    fn take(&mut self, sim: &Sim) {
        self.stack = self.stack.max(sim.stack());
        self.handler_instructions = self.handler_instructions.max(sim.longest_handler);
        self.landed += sim.overflows_landed;
        self.last_new = self.last_new.max(sim.last_new);
    }
}

struct Linked {
    main: u32,
    ends: Ends,
    layout: Layout,
}

/// Byte addresses.
#[derive(Clone, Copy, Default)]
struct Ends {
    unhandled: u32,
    halt: u32,
}

impl Linked {
    fn of(elf: &Elf) -> Result<Self, String> {
        let symbol = |name| symbol(elf, name);
        let sram = |name| -> Result<u16, String> { Ok((symbol(name)? - DATA_SPACE) as u16) };
        Ok(Self {
            main: symbol("main")? / 2,
            ends: Ends {
                unhandled: symbol("__unhandled_interrupt")?,
                halt: layout::halt(elf.symbols(), |name| elf.symbol(name))?,
            },
            layout: Layout {
                data: sram("__data_start")?..sram("__data_end")?,
                bss: sram("__bss_start")?..sram("__bss_end")?,
                load: symbol("__data_load")? as u16,
            },
        })
    }
}

fn symbol(elf: &Elf, name: &str) -> Result<u32, String> {
    elf.symbol(name)
        .ok_or_else(|| format!("no `{name}` symbol"))
}

pub fn check(elf: &Elf, examples: bool) -> Result<Report, String> {
    let linked = Linked::of(elf)?;
    let image = elf.flash_image();
    let signed = crc::sign(&image, &stamp::TEST)?;
    let reset = symbol(elf, "__reset")?;

    let power_on = || -> Result<Sim, String> {
        let mut sim = Sim::new(signed.clone(), linked.layout.bss.end, linked.ends)?;
        sim.run_to(linked.main, BOOT_BUDGET)
            .map_err(|e| format!("startup: {e}"))?;
        Ok(sim)
    };
    let sim = power_on()?;
    check_startup(&sim.chip, &linked.layout).map_err(|e| format!("startup: {e}"))?;
    startup_with_statics(&image, reset, &linked)
        .map_err(|e| format!("startup with statics: {e}"))?;
    found_lit(Sim::new(
        signed.clone(),
        linked.layout.bss.end,
        linked.ends,
    )?)
    .map_err(|e| format!("starting with PB2 high: {e}"))?;
    boot_order(Sim::new(
        signed.clone(),
        linked.layout.bss.end,
        linked.ends,
    )?)
    .map_err(|e| format!("start-up order: {e}"))?;
    unhandled_interrupt(power_on()?)
        .map_err(|e| format!("with an interrupt that has no handler: {e}"))?;
    let examples = if examples { EXAMPLES } else { &[] };
    let faults: [(FaultCheck, &str); 4] = [
        (slow_ticks, "with the slowest ticks"),
        (oscillator_stops, "with the 32 kHz oscillator stopped"),
        (adc_never_answers, "with a dead ADC"),
        (switch_stuck, "with the switch stuck down"),
    ];
    let power_on = &power_on;
    let reports: Vec<Result<Report, String>> = thread::scope(|scope| {
        let faults = faults.map(|(check, name)| {
            scope.spawn(move || {
                let mut report = Report::default();
                check(power_on()?, &mut report).map_err(|e| format!("{name}: {e}"))?;
                Ok(report)
            })
        });
        // Spawning every run before joining any is what runs them in parallel.
        let runs: Vec<_> = examples
            .iter()
            .map(Some)
            .chain([None])
            .map(|example| scope.spawn(move || both_ways(power_on, example)))
            .collect();
        faults
            .into_iter()
            .chain(runs)
            .map(|run| run.join().unwrap_or_else(|_| Err("a run panicked".into())))
            .collect()
    });

    let mut report = Report::default();
    for run in reports {
        report.merge(&run?);
    }
    if report.off_instructions == 0 {
        return Err("never powered down".into());
    }
    if report.landed == 0 {
        return Err("no overflow ever landed mid-pass, so this checked nothing".into());
    }
    Ok(report)
}

type FaultCheck = fn(Sim, &mut Report) -> Result<(), String>;

const LANDING_MARGIN: u32 = 2 * TICKS_PER_SECOND as u32;

/// Overflows only land on the first time each instruction's reached, so past
/// the first run's last new code the second would only repeat it.
fn both_ways(
    power_on: &impl Fn() -> Result<Sim, String>,
    example: Option<&Example>,
) -> Result<Report, String> {
    let mut report = run(power_on, example, Landing::Watch)?;
    let until = report.last_new + LANDING_MARGIN;
    report.merge(&run(power_on, example, Landing::Until(until))?);
    Ok(report)
}

fn run(
    power_on: &impl Fn() -> Result<Sim, String>,
    example: Option<&Example>,
    landing: Landing,
) -> Result<Report, String> {
    let mut sim = power_on()?;
    sim.landing = landing;
    let landing = matches!(landing, Landing::Until(_));
    let mut report = Report::default();
    let result = match example {
        Some(example) => {
            ui_example(sim, example, &mut report).map_err(|e| format!("{}: {e}", example.name))
        }
        None => against_the_library(sim, &mut report).map_err(|e| format!("random switching: {e}")),
    };
    match (result, landing) {
        (Ok(()), _) => Ok(report),
        (Err(e), true) => Err(format!("with overflows mid-pass: {e}")),
        (Err(e), false) => Err(e),
    }
}

const RANDOM_TICKS: usize = 20_000;

fn ui_example(mut sim: Sim, example: &Example, report: &mut Report) -> Result<(), String> {
    // The level is the library's alone, so the run that wasn't cut short has
    // checked it.
    let Some(level) = in_step(&mut sim, testing::samples(example.actions()), report)? else {
        return Ok(());
    };
    if example.expect.holds(level) {
        Ok(())
    } else {
        Err(format!(
            "expected {:?}, but it ends at level {level}",
            example.expect
        ))
    }
}

fn against_the_library(mut sim: Sim, report: &mut Report) -> Result<(), String> {
    let samples = RandomSwitching::new(0x5EED)
        .flat_map(|(pressed, ticks)| iter::repeat_n(pressed, ticks.into()))
        .take(RANDOM_TICKS)
        .map(|pressed| Sample {
            pressed,
            scene: Scene::NORMAL,
        });
    in_step(&mut sim, samples, report).map(drop)
}

fn lit(sim: &mut Sim, ticks: u16, report: &mut Report) -> Result<(), String> {
    let script = [Action::Clicks(1), Action::Wait(ticks)];
    in_step(sim, testing::samples(script), report)?;
    if !sim.chip.enabled_high() {
        return Err("a click from off left PB2 low, so there's no light to check".into());
    }
    Ok(())
}

/// The 32 kHz oscillator 30% slow and the 20 MHz one 5% fast (tables 36-13 and
/// 36-12, full operating range, from characterisation).
const SLOWEST_TICK_CYCLES: u64 = TICK_CYCLES * 105 / 70;
const DEADLINE_CYCLES: u64 = TICK_DEADLINE_CYCLES as u64 * PWM_CYCLE;
// Only a stopped oscillator may trip the deadline, since it stops the chip for
// good.
const _: () = assert!(4 * SLOWEST_TICK_CYCLES < DEADLINE_CYCLES);

/// The model's watchdog counts PIT periods, so a slower PIT doesn't move it.
fn slow_ticks(mut sim: Sim, report: &mut Report) -> Result<(), String> {
    sim.chip.tick_cycles = SLOWEST_TICK_CYCLES;
    lit(&mut sim, 2 * TICKS_PER_SECOND, report)
}

fn adc_never_answers(sim: Sim, report: &mut Report) -> Result<(), String> {
    let seconds = |s: usize| s * usize::from(TICKS_PER_SECOND);
    let script = [
        Action::Clicks(1),
        Action::Wait(10 * TICKS_PER_SECOND),
        Action::Clicks(1),
        Action::Wait(2 * TICKS_PER_SECOND),
    ];
    let levels = std::cell::RefCell::new(Vec::new());
    let mut shadow = Rig::default();
    let samples = testing::samples(script).map(|sample| {
        let sample = Sample {
            scene: Scene {
                adc_dead: true,
                ..sample.scene
            },
            ..sample
        };
        shadow.tick_in(sample.pressed, &sample.scene);
        levels.borrow_mut().push(shadow.level());
        sample
    });
    let mut sim = sim;
    let end = in_step(&mut sim, samples, report)?;
    let levels = levels.into_inner();
    if end != Some(0) {
        return Err(format!("ends at level {end:?}, not off"));
    }
    let lit = levels
        .iter()
        .position(|&level| level > 0)
        .ok_or("the light never came on")?;
    let out = lit + seconds(4);
    if !levels.get(out..).is_some_and(|l| l.iter().all(|&l| l == 0)) {
        return Err(format!(
            "the light isn't out and staying out from 4 s after it came on ({:?} at {out})",
            levels.get(out)
        ));
    }
    Ok(())
}

fn switch_stuck(mut sim: Sim, report: &mut Report) -> Result<(), String> {
    let samples = iter::repeat_n(
        Sample {
            pressed: true,
            scene: Scene::NORMAL,
        },
        20_000,
    );
    let end = in_step(&mut sim, samples, report)?;
    if end != Some(ts26::ui::FLOOR) {
        return Err(format!("ends at level {end:?}, not the floor"));
    }
    Ok(())
}

const HALT_BUDGET: u64 = 100;
const DOWN_PERIODS: u64 = 4 * TICKS_PER_SECOND as u64;

/// The stopped oscillator takes the tick and the watchdog with it, so
/// `hw::wait_for_tick`'s deadline is all that's left. A tick lost with the
/// oscillator running ends in a watchdog reset instead, which can't run here.
fn oscillator_stops(mut sim: Sim, report: &mut Report) -> Result<(), String> {
    lit(&mut sim, 0, report)?;
    sim.chip.osc32k_stopped = true;
    let stopped = sim.chip.now;
    while sim.chip.enabled_high() {
        sim.step()?;
        if sim.chip.now - stopped > DEADLINE_CYCLES + TICK_BUDGET {
            return Err(format!(
                "PB2's still high {} PWM cycles on, past the deadline of {TICK_DEADLINE_CYCLES}",
                (sim.chip.now - stopped) / PWM_CYCLE
            ));
        }
    }
    let took = sim.chip.now - stopped;
    // The first cycle of the wait is a part of one.
    if took < DEADLINE_CYCLES - PWM_CYCLE {
        return Err(format!(
            "PB2 dropped after {} PWM cycles, before the deadline of {TICK_DEADLINE_CYCLES}",
            took / PWM_CYCLE
        ));
    }

    for _ in 0..HALT_BUDGET {
        if sim.chip.down {
            break;
        }
        sim.step()?;
        if sim.chip.enabled_high() {
            return Err("dropped PB2, then raised it again".into());
        }
    }
    if !sim.chip.down {
        return Err(format!(
            "dropped PB2, but didn't stop for good within {HALT_BUDGET} instructions"
        ));
    }
    if !sim.chip.powering_down() {
        return Err("stopped for good in idle, which leaves the main clock running".into());
    }
    let until = sim.chip.now + DOWN_PERIODS * TICK_CYCLES;
    while sim.chip.now < until {
        sim.step()?;
        if !sim.cpu.asleep || sim.chip.enabled_high() {
            return Err("woke up or raised PB2 after stopping for good".into());
        }
    }
    // `hw::halt` is a call deeper than the loop.
    report.stack = report.stack.max(sim.stack());
    Ok(())
}

/// Nothing in the firmware touches TCB0.
const VECTOR_UNHANDLED: u8 = regs::VECTOR_TCB0_INT as u8;

/// A real chip's watchdog then resets it, which can't run here, so the run
/// stops one PIT period short of the bite. Forcing it on the pass after a
/// `wdr` leaves the most periods to watch.
fn unhandled_interrupt(mut sim: Sim) -> Result<(), String> {
    if sim.handled & 1 << VECTOR_UNHANDLED != 0 {
        return Err(format!(
            "{} has a handler now, so pick another vector to force",
            vector_name(VECTOR_UNHANDLED)
        ));
    }
    lit(&mut sim, TICKS_PER_SECOND / 4, &mut Report::default())?;
    // A pass that feeds leaves the count lower than the pass before it did.
    let mut fed = false;
    for _ in 0..=FEED_TICKS {
        let before = sim.chip.wdt.ticks;
        sim.tick(false)?;
        if sim.chip.wdt.ticks < before {
            fed = true;
            break;
        }
    }
    if !fed {
        return Err(format!(
            "no wdr within {FEED_TICKS} ticks of the light coming on"
        ));
    }
    sim.chip.force_interrupt(VECTOR_UNHANDLED);
    let mut steps = 0;
    while !sim.chip.down {
        if steps == HALT_BUDGET {
            return Err(format!(
                "didn't stop for good within {HALT_BUDGET} instructions of {} being forced",
                vector_name(VECTOR_UNHANDLED)
            ));
        }
        sim.step()?;
        steps += 1;
    }
    if sim.forced != Some(VECTOR_UNHANDLED) || sim.cpu.handling != sim.forced {
        return Err("stopped for good without taking the forced interrupt".into());
    }
    if !sim.abandoned {
        return Err("stopped for good without going through hw::halt".into());
    }
    if sim.chip.enabled_high() || sim.chip.duty() != 0 {
        return Err(format!(
            "stopped for good with PB2 {} and a duty of {}",
            if sim.chip.enabled_high() {
                "high"
            } else {
                "low"
            },
            sim.chip.duty()
        ));
    }
    if !sim.chip.powering_down() {
        return Err("stopped for good in idle, which leaves the main clock running".into());
    }

    let periods = sim.chip.wdt_periods_left();
    if periods == 0 {
        return Err(
            "stopped for good with the watchdog about to bite, so nothing's left to watch".into(),
        );
    }
    let (ticks, until) = (
        sim.chip.ticks,
        sim.chip.next_pit + (periods - 1) * sim.chip.tick_cycles,
    );
    while sim.chip.now < until {
        sim.step()?;
        if !sim.cpu.asleep || sim.cpu.handling != sim.forced || sim.chip.enabled_high() {
            return Err("woke up, took an interrupt or raised PB2 after stopping for good".into());
        }
    }
    if sim.chip.ticks == ticks {
        return Err(
            "no PIT period went by after stopping for good, so this checked nothing".into(),
        );
    }
    Ok(())
}

/// `None` if `Landing::Until` cut it short.
fn in_step(
    sim: &mut Sim,
    samples: impl IntoIterator<Item = Sample>,
    report: &mut Report,
) -> Result<Option<u8>, String> {
    // The firmware reads the fuses before its first tick.
    let mut samples = samples.into_iter().peekable();
    let power_on = World::from(samples.peek().map_or(Scene::NORMAL, |s| s.scene));
    sim.chip.world = power_on;
    if power_on.scene.corrupt_flash {
        // The checksum's byte is never code, however big the firmware gets.
        let byte = sim
            .chip
            .flash
            .get_mut(crc::CHECKSUM_AT)
            .ok_or("can't corrupt flash: the image isn't padded to all of flash")?;
        *byte ^= 0x01;
    }
    sim.tick(false).map_err(|e| format!("hw::init: {e}"))?;
    sim.chip
        .clocked_as_modelled()
        .map_err(|e| format!("hw::init: {e}"))?;
    if sim.chip.wdt_open() == 0 {
        return Err("hw::start_watchdog never turned the watchdog on".into());
    }
    let mut rig = Rig::new(power_on.faults());
    let mut was_resting = false;
    let mut feeder = Feeder::new();
    let mut finished = true;
    for (tick, sample) in samples.enumerate() {
        let at = |e: String| format!("tick {tick}: {e}");
        sim.tick_no = tick as u32;
        let landing = match sim.landing {
            Landing::Until(until) if sim.tick_no > until => {
                finished = false;
                break;
            }
            Landing::Until(_) => true,
            Landing::Off | Landing::Watch => false,
        };
        sim.chip.world = World::from(sample.scene);
        let power_downs = sim.chip.power_downs;
        let took = sim.tick(sample.pressed).map_err(at)?;
        // A real tick never sees that many handlers on top of the pass.
        if !landing {
            report.loop_instructions = report.loop_instructions.max(took);
        }
        let powered_down = sim.chip.power_downs > power_downs;
        if powered_down && was_resting {
            report.off_instructions = report.off_instructions.max(took);
        }
        rig.tick_in(sample.pressed, &sample.scene);
        sim.chip.shows(rig.torch.output()).map_err(at)?;
        if sim.chip.aux() != board::aux_pin(rig.torch.aux()) {
            return Err(at(format!(
                "the aux pins are {:#06b}, but the library says {:?}",
                sim.chip.aux(),
                rig.torch.aux()
            )));
        }
        if sim.chip.aux_bright() != board::aux_bright(rig.torch.aux()) {
            return Err(at(format!(
                "the bright aux pins are {:#06b}, but the library says {:?}",
                sim.chip.aux_bright(),
                rig.torch.aux()
            )));
        }
        was_resting = rig.torch.resting();
        // The sleep that ended this pass answers the state the pass left, so
        // the feeder ticks after it, as in `main`.
        feeder.tick();
        let power_down = feeder.power_down(was_resting);
        if powered_down != power_down {
            return Err(at(format!(
                "the library says {}, but the firmware {}",
                if power_down { "power down" } else { "idle" },
                if powered_down {
                    "powered down"
                } else {
                    "idled"
                },
            )));
        }
    }
    report.take(sim);
    Ok(finished.then(|| rig.level()))
}

/// The .rodata table, the .data static's starting values, then a byte it
/// doesn't touch.
const PROBE_EXPECTED: [u8; 7] = [0x12, 0x34, 0x56, 0x9A, 0xBC, 0xDE, 0];

pub fn probe(elf: &Elf) -> Result<(), String> {
    let linked = Linked::of(elf)?;
    let layout = &linked.layout;
    let rodata = elf.section(".rodata").ok_or("no .rodata section")?;
    if rodata.is_empty() || layout.data.is_empty() || layout.bss.is_empty() {
        return Err(".rodata, .data or .bss is empty, so there's nothing to check".into());
    }
    let mut sim = Sim::new(elf.flash_image(), layout.bss.end, linked.ends)?;
    sim.run_to(linked.main, BOOT_BUDGET)
        .and_then(|()| check_startup(&sim.chip, layout))
        .map_err(|e| format!("startup: {e}"))?;
    sim.run_to(symbol(elf, "link_probe_done")? / 2, BOOT_BUDGET)?;

    let copy = (symbol(elf, "LINK_PROBE_COPY")? - DATA_SPACE) as u16;
    let got = sim.chip.sram(&(copy..copy + PROBE_EXPECTED.len() as u16));
    if got != PROBE_EXPECTED {
        return Err(format!(
            "LINK_PROBE_COPY holds {got:02x?}, expected {PROBE_EXPECTED:02x?}"
        ));
    }
    Ok(())
}

/// examples/guards.rs asks for power-down four times lit and once off.
pub fn guards(elf: &Elf) -> Result<(), String> {
    let linked = Linked::of(elf)?;
    let mut sim = Sim::new(elf.flash_image(), linked.layout.bss.end, linked.ends)?;
    sim.run_to(
        symbol(elf, "guards_done")? / 2,
        BOOT_BUDGET + 8 * TICK_BUDGET,
    )?;
    match sim.chip.power_downs {
        1 => Ok(()),
        n => Err(format!(
            "powered down {n} times, but only its one wait with the light off should"
        )),
    }
}

/// Data-space addresses.
struct Layout {
    data: Range<u16>,
    bss: Range<u16>,
    load: u16,
}

fn check_startup(chip: &Chip, layout: &Layout) -> Result<(), String> {
    let from = usize::from(layout.load.wrapping_sub(MAPPED));
    let len = usize::from(layout.data.end - layout.data.start);
    let initial = chip
        .flash
        .get(from..from + len)
        .ok_or(".data's initial values are outside flash")?;
    if chip.sram(&layout.data) != initial {
        return Err(".data in SRAM doesn't match its initial values in flash".into());
    }
    if let Some((i, b)) = chip
        .sram(&layout.bss)
        .iter()
        .enumerate()
        .find(|&(_, &b)| b != 0)
    {
        return Err(format!(
            ".bss isn't zeroed: {:#06x} holds {b:#04x}",
            usize::from(layout.bss.start) + i
        ));
    }
    Ok(())
}

/// `reset` is a byte address.
fn startup_with_statics(image: &[u8], reset: u32, linked: &Linked) -> Result<(), String> {
    let values = [0x11, 0x22, 0x33, 0x44, 0x55];
    let mut image = image.to_vec();
    image.resize(image.len().next_multiple_of(2), 0xFF);
    let load = MAPPED + image.len() as u16;
    image.extend(values);

    // An odd start, away from the edges of SRAM, so off-by-ones show.
    let start = SRAM.start + 0x11;
    let moved = Layout {
        data: start..start + 5,
        bss: start + 5..start + 12,
        load,
    };
    patch_startup(&mut image, reset as usize, &linked.layout, &moved)?;

    let mut sim = Sim::new(image, moved.bss.end, linked.ends)?;
    sim.run_to(linked.main, BOOT_BUDGET)?;
    check_startup(&sim.chip, &moved)?;
    for edge in [moved.data.start - 1, moved.bss.end] {
        if sim.chip.sram(&(edge..edge + 1)) != [JUNK] {
            return Err(format!("wrote to {edge:#06x}, outside .data and .bss"));
        }
    }
    Ok(())
}

fn patch_startup(
    image: &mut [u8],
    reset: usize,
    linked: &Layout,
    moved: &Layout,
) -> Result<(), String> {
    // In src/avr/rt.rs's order.
    let loads = |l: &Layout| -> Vec<(u8, u8)> {
        [
            (26, l.data.start),
            (30, l.load),
            (24, l.data.end),
            (26, l.bss.start),
            (24, l.bss.end),
        ]
        .into_iter()
        .flat_map(|(r, v)| {
            let [lo, hi] = v.to_le_bytes();
            [(r, lo), (r + 1, hi)]
        })
        .collect()
    };

    let mut found = Vec::new();
    let mut at = reset;
    loop {
        let w = word(image, at).ok_or("no call to main after __reset")?;
        if w & 0xFE0E == 0x940E {
            break; // call main
        }
        if w >> 12 == 0xE {
            let k = ((w >> 4) & 0xF0 | w & 0x0F) as u8;
            found.push((at, (16 + ((w >> 4) & 0x0F)) as u8, k));
        }
        at += if is_two_words(w) { 4 } else { 2 };
    }

    let actual: Vec<(u8, u8)> = found.iter().map(|&(_, r, k)| (r, k)).collect();
    if actual != loads(linked) {
        return Err(
            "src/avr/rt.rs's startup doesn't load the registers sim.rs expects; update `patch_startup`"
                .into(),
        );
    }
    for (&(at, r, _), (_, k)) in found.iter().zip(loads(moved)) {
        image[at..at + 2].copy_from_slice(&ldi(r, k).to_le_bytes());
    }
    Ok(())
}

/// Not a model of a stray jump to the reset vector, since only PORTB starts
/// dirty and the rest of the chip is as a reset leaves it.
fn found_lit(mut sim: Sim) -> Result<(), String> {
    sim.chip.io[reg::VPORTB_DIR] = ENABLE | PWM;
    sim.chip.io[reg::VPORTB_OUT] = ENABLE;
    for _ in 0..BOOT_BUDGET {
        if sim.chip.register_writes == 0 {
            sim.step()?;
        } else if sim.chip.io[reg::VPORTB_OUT] & ENABLE == 0 {
            return Ok(());
        } else {
            return Err(
                "the first register it writes leaves PB2 set in VPORTB.OUT, so the boost \
                 converter stays on: clear OUT before DIR, and before anything else"
                    .into(),
            );
        }
    }
    Err(format!(
        "wrote no register within {BOOT_BUDGET} instructions"
    ))
}

fn boot_order(mut sim: Sim) -> Result<(), String> {
    for _ in 0..BOOT_BUDGET {
        if sim.chip.tick_polled {
            return sim.chip.boot.in_order();
        }
        sim.step()?;
    }
    Err(format!(
        "didn't wait for a tick within {BOOT_BUDGET} instructions"
    ))
}

/// `unhandled` is a word address.
fn handled(image: &[u8], unhandled: u32) -> u32 {
    (1..VECTORS).fold(0, |mask, v| {
        let at = v * VECTOR_WORDS as usize * 2;
        if jmp_target(image, at).is_some_and(|target| target / 2 != unhandled) {
            mask | 1 << v
        } else {
            mask
        }
    })
}

/// With the return address pushed.
struct Landed {
    r: [u8; 32],
    sreg: u8,
    sp: u16,
}

struct Sim {
    cpu: Cpu,
    chip: Chip,
    stack_floor: u16,
    lowest_sp: u16,
    /// Unlike `Chip::now`, stops while asleep.
    executed: u64,
    /// Word address.
    unhandled: u32,
    /// Word address.
    halt: u32,
    handled: u32,
    forced: Option<u8>,
    /// The running handler has reached `hw::halt`, so it'll never return.
    abandoned: bool,
    landed: Option<Landed>,
    handler_steps: u64,
    longest_handler: u64,
    landing: Landing,
    reached: Vec<bool>,
    tick_no: u32,
    last_new: u32,
    overflows_landed: u32,
}

#[derive(Clone, Copy)]
enum Landing {
    Off,
    Watch,
    Until(u32),
}

impl Sim {
    fn new(image: Vec<u8>, bss_end: u16, ends: Ends) -> Result<Self, String> {
        if image.len() > FLASH_SIZE {
            return Err(format!("{} bytes won't fit in flash", image.len()));
        }
        let cpu = Cpu::new(&SRAM);
        let unhandled = ends.unhandled / 2;
        let handled = handled(&image, unhandled);
        let chip = Chip::new(image);
        Ok(Self {
            lowest_sp: cpu.sp,
            cpu,
            handled,
            chip,
            stack_floor: bss_end - 1,
            executed: 0,
            unhandled,
            halt: ends.halt / 2,
            forced: None,
            abandoned: false,
            landed: None,
            handler_steps: 0,
            longest_handler: 0,
            landing: Landing::Off,
            reached: vec![false; FLASH_SIZE / 2],
            tick_no: 0,
            last_new: 0,
            overflows_landed: 0,
        })
    }

    /// A landed overflow isn't a real BOTTOM, and the handler can overwrite a
    /// carry in CMP0BUF before BOTTOM takes it, so neither the sum in
    /// `Chip::shows` nor the zero-duty count holds across one.
    fn land(&mut self) {
        if matches!(self.landing, Landing::Off) {
            return;
        }
        let chip = &mut self.chip;
        let enabled = chip.enabled & 1 << VECTOR_TCA0_OVF != 0;
        if self.cpu.handling.is_some() || self.cpu.asleep || !enabled || chip.tca.ovf {
            return;
        }
        if let Some(seen @ false) = self.reached.get_mut(self.cpu.pc as usize) {
            *seen = true;
            self.last_new = self.tick_no;
            if let Landing::Until(_) = self.landing {
                chip.tca.ovf = true;
                chip.tca.ovf_up_at = chip.now;
                chip.tca.settled = 0;
                chip.tca.zero_run = 0;
                self.overflows_landed += 1;
            }
        }
    }

    const fn stack(&self) -> u16 {
        SRAM.end - 1 - self.lowest_sp
    }

    fn step(&mut self) -> Result<(), String> {
        let pc = self.cpu.pc;
        let awake = !self.cpu.asleep;
        let handling = self.cpu.handling;
        // `hw::halt` gets the main loop's rules, wherever it was reached from.
        self.chip.in_handler = handling.is_some() && !self.abandoned;
        self.chip.interrupts_on = self.cpu.flag(I);
        self.cpu.step(&mut self.chip)?;
        // Only a reset wakes a chip with no interrupt enabled (§11.3.1), and
        // figure 13-1 has SREG.I gate the wake-up too. Neither says the other's
        // enough, so stopping for good wants both.
        if self.chip.down && self.cpu.flag(I) {
            return Err(format!(
                "{:#06x}: stops for good with SREG.I still set, which leaves the \
                 interrupt enables alone keeping it down",
                pc * 2
            ));
        }
        self.watch_handler(handling)?;
        if self.cpu.handling.is_some() && self.cpu.pc == self.halt {
            self.abandon();
        }
        if awake {
            self.executed += 1;
            self.chip.elapse()?;
        }
        self.lowest_sp = self.lowest_sp.min(self.cpu.sp);
        if self.cpu.sp < self.stack_floor {
            return Err(format!("{:#06x}: the stack ran into .bss", pc * 2));
        }
        // The real chip would `reti` with the flag still up and take it again
        // forever, so this fails at the enable, before it fires.
        if self.cpu.flag(I) {
            let orphans = self.chip.enabled & !self.handled;
            if orphans != 0 {
                return Err(format!(
                    "{:#06x}: enables {} with SREG.I set, but its vector goes to \
                     __unhandled_interrupt",
                    pc * 2,
                    vector_name(orphans.trailing_zeros() as u8)
                ));
            }
        }
        if self.cpu.pc == self.unhandled
            && let Some(vector) = self.cpu.handling
            && self.forced != Some(vector)
        {
            return Err(format!(
                "takes {}, whose vector goes to __unhandled_interrupt",
                vector_name(vector)
            ));
        }
        self.land();
        Ok(())
    }

    fn watch_handler(&mut self, before: Option<u8>) -> Result<(), String> {
        match (before, self.cpu.handling) {
            (None, Some(vector)) => {
                if self.chip.forced == Some(vector) {
                    self.chip.forced = None;
                    self.forced = Some(vector);
                }
                self.chip.interrupted = self.chip.tca.temp.map(|(low, _)| low);
                self.handler_steps = 0;
                self.landed = Some(Landed {
                    r: self.cpu.r,
                    sreg: self.cpu.sreg,
                    sp: self.cpu.sp,
                });
            }
            (Some(vector), None) => {
                self.chip.interrupted = None;
                if vector == VECTOR_TCA0_OVF {
                    self.longest_handler = self.longest_handler.max(self.handler_steps);
                }
                if let Some(landed) = self.landed.take() {
                    self.gave_back(&landed)
                        .map_err(|e| format!("the handler for {}: {e}", vector_name(vector)))?;
                }
            }
            // A handler in `hw::halt` has stopped being one in all but
            // LVL0EX, and sleeps from there until a reset.
            (Some(_), Some(_)) if self.abandoned => {}
            (Some(vector), Some(_)) => {
                self.handler_steps += 1;
                if self.handler_steps > HANDLER_BUDGET {
                    return Err(format!(
                        "the handler for {} ran past {HANDLER_BUDGET} instructions, \
                         which might not fit in a PWM cycle",
                        vector_name(vector)
                    ));
                }
            }
            (None, None) => {}
        }
        Ok(())
    }

    /// `hw::halt` never returns, so the main loop never writes the high byte
    /// of an access the handler interrupted, and `halt` may overwrite TEMP.
    const fn abandon(&mut self) {
        self.abandoned = true;
        self.chip.interrupted = None;
        self.chip.tca.temp = None;
        self.landed = None;
    }

    fn gave_back(&self, landed: &Landed) -> Result<(), String> {
        let changed = (0..32).find(|&n| self.cpu.r[n] != landed.r[n]);
        if let Some(n) = changed {
            return Err(format!(
                "returns with r{n} at {:#04x}, not the {:#04x} it found",
                self.cpu.r[n], landed.r[n]
            ));
        }
        if self.cpu.sreg != landed.sreg {
            return Err(format!(
                "returns with SREG at {:#04x}, not the {:#04x} it found",
                self.cpu.sreg, landed.sreg
            ));
        }
        if self.cpu.sp != landed.sp.wrapping_add(2) {
            return Err(format!(
                "returns with SP at {:#06x}, having found it at {:#06x}",
                self.cpu.sp,
                landed.sp.wrapping_add(2)
            ));
        }
        Ok(())
    }

    /// `target` is a word address.
    fn run_to(&mut self, target: u32, budget: u64) -> Result<(), String> {
        for _ in 0..budget {
            if self.cpu.pc == target {
                return Ok(());
            }
            self.step()?;
        }
        Err(format!(
            "didn't reach {:#06x} within {budget} instructions",
            target * 2
        ))
    }

    fn tick(&mut self, pressed: bool) -> Result<u64, String> {
        self.chip.set_switch(pressed)?;
        let (start, ticks) = (self.executed, self.chip.ticks);
        self.chip.tick_polled = false;
        let mut pass_end = None;
        while self.chip.ticks == ticks {
            self.step()?;
            if self.chip.down {
                return Err(
                    "sleeps with no interrupt enabled, so only a reset wakes it \
                     (section 11.3.1): it's stopped for good"
                        .into(),
                );
            }
            if self.chip.tick_polled {
                pass_end.get_or_insert(self.executed);
            }
            let ran = pass_end.unwrap_or(self.executed) - start;
            if ran > TICK_BUDGET {
                return Err(format!(
                    "a pass of the main loop ran past {TICK_BUDGET} instructions, \
                     so it hangs or misses 64 Hz ticks"
                ));
            }
        }
        Ok(pass_end.unwrap_or(self.executed) - start)
    }
}

/// Timed in PIT periods, since the two share an oscillator.
#[derive(Default)]
struct Wdt {
    ticks: u32,
    windowed: bool,
    busy: u8,
}

const fn wdt_clocks(setting: u8) -> u32 {
    match setting {
        0 => 0,
        n => 8 << (n - 1),
    }
}

#[derive(Default)]
struct Boot {
    flags_cleared: Option<u64>,
    flash_checked: Option<u64>,
    fuses_read: Option<u64>,
    set_up: Option<u64>,
    pit_on: Option<u64>,
    watchdog_on: Option<u64>,
}

impl Boot {
    fn write(&mut self, now: u64, a: usize, value: u8) {
        let first = |slot: &mut Option<u64>| _ = slot.get_or_insert(now);
        match a {
            reg::RSTCTRL_RSTFR => first(&mut self.flags_cleared),
            reg::CRCSCAN_CTRLA => first(&mut self.flash_checked),
            reg::CRCSCAN_CTRLB | reg::VPORTB_OUT | reg::VPORTB_DIR => {}
            _ => first(&mut self.set_up),
        }
        if a == reg::RTC_PITCTRLA && value & RTC_PITEN != 0 {
            first(&mut self.pit_on);
        }
        if a == reg::WDT_CTRLA {
            first(&mut self.watchdog_on);
        }
    }

    fn in_order(&self) -> Result<(), String> {
        let at = |when: Option<u64>, what: &str| when.ok_or_else(|| format!("never {what}"));
        let flags = at(self.flags_cleared, "cleared RSTCTRL.RSTFR")?;
        let flash = at(self.flash_checked, "started a CRC scan")?;
        let fuses = at(self.fuses_read, "read the fuses")?;
        let set_up = at(self.set_up, "set anything up")?;
        let pit = at(self.pit_on, "turned the PIT on")?;
        let watchdog = at(self.watchdog_on, "turned the watchdog on")?;
        let rules = [
            (
                flags < watchdog,
                "turns the watchdog on before clearing the reset flags, so a bite could \
                 reset the chip before `main` has read why it last reset",
            ),
            (
                flash < set_up,
                "sets the chip up before the CRC scan, so code nobody's checked has \
                 already run",
            ),
            (
                fuses < set_up,
                "sets the chip up before reading the fuses, so it's run on fuses nobody's \
                 checked",
            ),
            (
                pit < watchdog,
                "turns the watchdog on before the PIT, so its first timeout starts before \
                 there's a tick to feed it on",
            ),
        ];
        match rules.into_iter().find(|&(ok, _)| !ok) {
            Some((_, why)) => Err(why.into()),
            None => Ok(()),
        }
    }
}

struct CrcScan {
    started: Option<(u64, u8)>,
    busy_reads: u8,
    /// OK from reset, before any scan (§27.5.3), so the firmware has to wait
    /// for its own.
    status: u8,
}

#[derive(Default)]
struct Adc {
    last_setup: Option<u8>,
    result: u16,
    ready_at: Option<u64>,
    temp: Option<u8>,
}

struct Tca {
    per: u16,
    perbuf: u16,
    perbv: bool,
    cmp0: u16,
    cmp0buf: u16,
    cmp0bv: bool,
    /// One for all the 16-bit registers (§20.5.13).
    temp: Option<(usize, u8)>,
    next_bottom: u64,
    ovf: bool,
    ovf_up_at: u64,
    zero_run: u32,
    recent: [u16; DSM_WINDOW],
    at: usize,
    settled: usize,
}

struct Chip {
    flash: Vec<u8>,
    sram: Vec<u8>,
    io: Vec<u8>,
    now: u64,
    next_event: u64,
    unlocked_until: Option<u64>,
    /// The key's first store to the bus or `sleep` ends it (§8.5.7.1).
    ccp_hold: bool,
    pressed: bool,
    pit_busy: u8,
    pit_flag: bool,
    porta_flags: u8,
    next_pit: u64,
    tick_cycles: u64,
    /// The PIT and the WDT both count it (§23.12.7's reset value, §19.3.2).
    /// The firmware writes neither after start-up, so a sync left waiting on
    /// the oscillator isn't modelled.
    osc32k_stopped: bool,
    /// Only a reset wakes the chip (§11.3.1), and this one has no resets.
    down: bool,
    ticks: u32,
    tick_polled: bool,
    tca: Tca,
    wdt: Wdt,
    crc: CrcScan,
    adc: Adc,
    rstfr: u8,
    world: World,
    enabled: u32,
    forced: Option<u8>,
    in_handler: bool,
    interrupts_on: bool,
    interrupted: Option<usize>,
    power_downs: u32,
    register_writes: u32,
    boot: Boot,
}

impl Chip {
    fn new(flash: Vec<u8>) -> Self {
        Self {
            flash,
            sram: vec![JUNK; usize::from(SRAM.end - SRAM.start)],
            io: vec![0; 0x1000],
            now: 0,
            next_event: TICK_CYCLES,
            unlocked_until: None,
            ccp_hold: false,
            pressed: false,
            pit_busy: PIT_SYNC_READS,
            pit_flag: false,
            porta_flags: 0,
            next_pit: TICK_CYCLES,
            tick_cycles: TICK_CYCLES,
            osc32k_stopped: false,
            down: false,
            ticks: 0,
            tick_polled: false,
            tca: Tca {
                per: 0xFFFF,
                perbuf: 0xFFFF,
                perbv: false,
                cmp0: 0,
                cmp0buf: 0,
                cmp0bv: false,
                temp: None,
                next_bottom: 0,
                ovf: false,
                ovf_up_at: 0,
                zero_run: 0,
                recent: [0; DSM_WINDOW],
                at: 0,
                settled: 0,
            },
            wdt: Wdt::default(),
            crc: CrcScan {
                started: None,
                busy_reads: 0,
                status: CRCSCAN_OK,
            },
            adc: Adc::default(),
            rstfr: RSTCTRL_PORF,
            world: World::NORMAL,
            enabled: 0,
            forced: None,
            in_handler: false,
            interrupts_on: false,
            interrupted: None,
            power_downs: 0,
            register_writes: 0,
            boot: Boot::default(),
        }
    }

    /// A one-shot stand-in, kept out of `enabled`. A real stray interrupt stays
    /// enabled and flagged, and whether one blocked by LVL0EX still wakes
    /// `halt`'s `sleep` isn't in the datasheet. Either way PB2 is low first,
    /// so the worst case is spinning dark until the watchdog resets the chip.
    const fn force_interrupt(&mut self, vector: u8) {
        self.forced = Some(vector);
    }

    fn sram(&self, range: &Range<u16>) -> &[u8] {
        &self.sram[usize::from(range.start - SRAM.start)..usize::from(range.end - SRAM.start)]
    }

    fn enabled_high(&self) -> bool {
        self.io[reg::VPORTB_OUT] & self.io[reg::VPORTB_DIR] & ENABLE != 0
    }

    /// An input with its pull-up on glows dimly.
    fn aux(&self) -> u8 {
        let dir = self.io[reg::VPORTC_DIR];
        let pulled_up = (0..8).fold(0, |bits, pin| {
            let on = self.io[reg::PORTC_PIN0CTRL + pin] & PORT_PULLUPEN != 0;
            bits | u8::from(on) << pin
        });
        (self.io[reg::VPORTC_OUT] & dir | pulled_up & !dir) & AUX
    }

    fn aux_bright(&self) -> u8 {
        self.io[reg::VPORTC_OUT] & self.io[reg::VPORTC_DIR] & AUX
    }

    /// Wrong writes are refused already; this catches one that never happened.
    fn clocked_as_modelled(&self) -> Result<(), String> {
        if self.io[reg::CLKCTRL_MCLKCTRLB] != CLOCK_10MHZ {
            return Err("never set the clock to 10 MHz".into());
        }
        if !self.tca_running() {
            return Err("never started TCA0".into());
        }
        if self.tca.per != PWM_TOP {
            return Err(format!(
                "TCA0.PER is {} after the first tick, not {PWM_TOP}",
                self.tca.per
            ));
        }
        Ok(())
    }

    fn duty(&self) -> u16 {
        if self.io[reg::TCA0_CTRLB] & TCA_CMP0EN != 0 {
            self.tca.cmp0
        } else {
            0
        }
    }

    fn tca_running(&self) -> bool {
        self.io[reg::TCA0_CTRLA] == TCA_RUNNING
    }

    fn powering_down(&self) -> bool {
        self.io[reg::SLPCTRL_CTRLA] == SLEEP_POWER_DOWN | SLPCTRL_SEN
    }

    fn take_protected_write(&mut self, a: usize) -> Result<(), String> {
        if self.unlocked_until.is_none_or(|until| self.now > until) {
            return Err(format!(
                "writes {} without the CCP key in the four instructions before, \
                 and the chip ignores that write",
                name(a)
            ));
        }
        self.unlocked_until = None;
        Ok(())
    }

    /// The one place simulated time moves.
    fn advance_to(&mut self, to: u64) -> Result<(), String> {
        self.now = to;
        if self.now < self.next_event {
            return Ok(());
        }
        if self.tca_running() {
            while self.now >= self.tca.next_bottom {
                self.bottom()?;
            }
        }
        while self.now >= self.next_pit {
            self.next_pit += self.tick_cycles;
            if self.osc32k_stopped {
                continue;
            }
            if self.io[reg::RTC_PITCTRLA] & RTC_PITEN != 0 {
                self.pit_flag = true;
                self.ticks += 1;
            }
            self.wdt_period()?;
        }
        self.plan_next_event();
        Ok(())
    }

    fn wdt_open(&self) -> u32 {
        wdt_clocks(self.io[reg::WDT_CTRLA] & WDT_PERIOD)
    }

    /// 0 before the first `wdr` starts window mode (§19.3.3.2).
    fn wdt_closed(&self) -> u32 {
        if self.wdt.windowed {
            wdt_clocks((self.io[reg::WDT_CTRLA] & WDT_WINDOW) >> 4)
        } else {
            0
        }
    }

    fn wdt_periods_left(&self) -> u64 {
        let timeout = self.wdt_closed() + self.wdt_open();
        if timeout == 0 {
            return DOWN_PERIODS;
        }
        let last_safe = (timeout - 1) / WDT_CLOCKS_PER_TICK;
        u64::from(last_safe.saturating_sub(self.wdt.ticks)).min(DOWN_PERIODS)
    }

    /// Runs in every sleep mode (§19.3.4).
    fn wdt_period(&mut self) -> Result<(), String> {
        if self.wdt_open() == 0 {
            return Ok(());
        }
        self.wdt.ticks += 1;
        let clocks = self.wdt.ticks * WDT_CLOCKS_PER_TICK;
        let timeout = self.wdt_closed() + self.wdt_open();
        if clocks >= timeout {
            return Err(format!(
                "the watchdog went {} ticks ({clocks} WDT clocks) without wdr, against a \
                 {timeout}-clock timeout, so the chip would have reset",
                self.wdt.ticks
            ));
        }
        Ok(())
    }

    const fn crc_status(&mut self) -> u8 {
        match self.crc.started {
            Some((at, before)) if self.now <= at + CRC_START_INSTRUCTIONS => before,
            Some(_) if self.crc.busy_reads > 0 => {
                self.crc.busy_reads -= 1;
                CRCSCAN_BUSY
            }
            _ => self.crc.status,
        }
    }

    fn start_crc(&mut self) -> Result<(), String> {
        let ctrlb = self.io[reg::CRCSCAN_CTRLB];
        if ctrlb != CRCSCAN_FLASH_PRIORITY {
            return Err(format!(
                "starts a CRC scan with CRCSCAN.CTRLB {ctrlb:#04x}; only the whole-flash \
                 priority scan works with BOOTEND and APPEND at 0"
            ));
        }
        let before = self.crc_status();
        let flash = self.flash.iter().copied().chain(iter::repeat(0xFF));
        let sum = crc::crc16(flash.take(FLASH_SIZE));
        self.crc = CrcScan {
            started: Some((self.now, before)),
            busy_reads: CRC_BUSY_READS,
            status: if sum == 0 { CRCSCAN_OK } else { 0 },
        };
        Ok(())
    }

    fn converting(&self) -> bool {
        self.adc.ready_at.is_some_and(|at| self.now < at)
    }

    fn start_conversion(&mut self) -> Result<(), String> {
        if self.converting() {
            return Err("starts a conversion while one's still running".into());
        }
        let io = |r: usize| self.io[r];
        if io(reg::ADC0_CTRLA) != ADC_ENABLE {
            return Err(format!(
                "starts a conversion with ADC0.CTRLA {:#04x}; it wants the ADC enabled, \
                 single conversions and 10-bit results",
                io(reg::ADC0_CTRLA)
            ));
        }
        if io(reg::VREF_CTRLB) & VREF_ADC0REFEN == 0 {
            return Err(
                "starts a conversion with VREF.CTRLB.ADC0REFEN clear, so the reference \
                 may still be starting up"
                    .into(),
            );
        }
        if io(reg::ADC0_CALIB) != ADC_DUTY25 {
            return Err("starts a conversion without ADC0.CALIB at 25% duty".into());
        }
        let ctrlc = io(reg::ADC0_CTRLC);
        let clk_adc = CLK_PER >> (1 + u32::from(ctrlc & ADC_PRESC));
        if !(200_000..=1_500_000).contains(&clk_adc) {
            return Err(format!(
                "runs the ADC at {clk_adc} Hz, outside 200 kHz to 1.5 MHz (table 36-26)"
            ));
        }
        let vref = io(reg::VREF_CTRLA) & VREF_ADC0REFSEL;
        let refsel = ctrlc & ADC_REFSEL;
        if vref != VREF_1V1 || refsel != ADC_REFSEL_INTREF {
            return Err(format!(
                "converts against REFSEL {refsel:#04x} and ADC0REFSEL {vref:#04x}; both \
                 measurements want the internal 1.1 V"
            ));
        }
        if io(reg::ADC0_CTRLB) & ADC_SAMPNUM != ADC_SAMPNUM_ACC4 {
            return Err("converts without four samples accumulated".into());
        }
        let muxpos = io(reg::ADC0_MUXPOS);
        let channel = match muxpos {
            ADC_MUXPOS_AIN1 => {
                // §30.3.1.1: buffer off and nothing else, so never a pull-up.
                let pa1 = io(reg::PORTA_PIN0CTRL + BATTERY_PIN);
                if pa1 != ISC_INPUT_DISABLE {
                    return Err(format!(
                        "converts AIN1 with PORTA.PIN1CTRL at {pa1:#04x}: PA1 wants its \
                         digital input buffer off and nothing else (section 30.3.1.1)"
                    ));
                }
                Channel::Voltage
            }
            ADC_MUXPOS_TEMPSENSE => {
                // §30.3.2.6: SAMPCAP, and INITDLY and SAMPLEN at least 32 µs.
                let initdly = match io(reg::ADC0_CTRLD) >> 5 {
                    0 => 0,
                    n => 8u32 << n,
                };
                let samplen = u32::from(io(reg::ADC0_SAMPCTRL) & ADC_SAMPLEN);
                let need = clk_adc.div_ceil(1_000_000 / 32);
                if ctrlc & ADC_SAMPCAP == 0 || initdly < need || samplen < need {
                    return Err(format!(
                        "reads the temperature sensor with INITDLY {initdly} and SAMPLEN \
                         {samplen}; it needs SAMPCAP and both at least {need} clocks"
                    ));
                }
                Channel::Temperature
            }
            _ => {
                return Err(format!(
                    "converts MUXPOS {muxpos:#04x}, which is neither AIN1 nor the \
                     temperature sensor"
                ));
            }
        };
        let setup = Some(muxpos);
        self.adc.result = if self.adc.last_setup == setup {
            self.world.reading(channel)
        } else {
            UNSETTLED
        };
        self.adc.last_setup = setup;
        self.adc.ready_at = Some(self.now + CONVERSION_INSTRUCTIONS);
        Ok(())
    }

    fn plan_next_event(&mut self) {
        self.next_event = if self.tca_running() {
            self.next_pit.min(self.tca.next_bottom)
        } else {
            self.next_pit
        };
    }

    fn elapse(&mut self) -> Result<(), String> {
        self.advance_to(self.now + 1)
    }

    /// Table 20-6.
    fn bottom(&mut self) -> Result<(), String> {
        let tca = &mut self.tca;
        // `now` has already moved past it.
        if !tca.ovf {
            tca.ovf_up_at = tca.next_bottom;
        }
        tca.ovf = true;
        if tca.perbv {
            tca.per = tca.perbuf;
            tca.perbv = false;
        }
        if tca.cmp0bv {
            tca.cmp0 = tca.cmp0buf;
            tca.cmp0bv = false;
        }
        tca.next_bottom += 2 * u64::from(tca.per.max(1));
        if tca.cmp0 > tca.per {
            return Err(format!(
                "CMP0 went live at {} against a PER of {}, which holds PB0 high for the cycle",
                tca.cmp0, tca.per
            ));
        }
        let duty = self.duty();
        let tca = &mut self.tca;
        tca.recent[tca.at] = duty;
        tca.at = (tca.at + 1) % DSM_WINDOW;
        tca.settled = (tca.settled + 1).min(DSM_WINDOW);
        if !self.enabled_high() || duty != 0 {
            self.tca.zero_run = 0;
            return Ok(());
        }
        self.tca.zero_run += 1;
        if self.tca.zero_run > ZERO_DUTY_CYCLES {
            return Err(format!(
                "PB2's been high with the duty at 0 for {} PWM cycles, longer than the \
                 delta-sigma's longest gap of {ZERO_DUTY_CYCLES}",
                self.tca.zero_run
            ));
        }
        Ok(())
    }

    fn dsm_sum(&self) -> Option<u32> {
        (self.tca.settled == DSM_WINDOW)
            .then(|| self.tca.recent.iter().map(|&c| u32::from(c)).sum())
    }

    fn shows(&self, output: Output) -> Result<(), String> {
        if self.enabled_high() != output.enable {
            return Err(format!(
                "PB2 is {}, but the library says {output:?}",
                self.enabled_high()
            ));
        }
        if !output.enable {
            return Ok(());
        }
        let want = u32::from(output.dsm);
        match self.dsm_sum() {
            Some(sum) if sum.abs_diff(want) > DSM_SLACK => Err(format!(
                "the last {DSM_WINDOW} PWM cycles add up to {sum}, but the library says {want}"
            )),
            _ => Ok(()),
        }
    }

    /// INVEN would flip both VPORTA.IN and which edge is which (§16.3.2.2),
    /// and the model does neither, so it refuses it.
    fn switch_ctrl(&self) -> u8 {
        self.io[reg::PORTA_PIN0CTRL + SWITCH_PIN]
    }

    /// §16.5.10. A press that's over before the chip wakes from power-down
    /// can wake it with no flag (table 16-3's note 1), which isn't modelled.
    fn set_switch(&mut self, pressed: bool) -> Result<(), String> {
        if pressed == self.pressed {
            return Ok(());
        }
        self.pressed = pressed;
        let sense = match self.switch_ctrl() & PORT_ISC {
            ISC_INTDISABLE | ISC_INPUT_DISABLE => false,
            ISC_BOTHEDGES => true,
            ISC_RISING => !pressed,
            ISC_FALLING => pressed,
            isc => {
                return Err(format!(
                    "PA5's ISC is {isc:#x}, which the simulator doesn't model"
                ));
            }
        };
        if sense {
            self.porta_flags |= SWITCH;
        }
        Ok(())
    }

    fn interrupts(&self, senses: impl Fn(usize, u8) -> bool, tca: bool) -> u32 {
        let port = |base: usize, vector: u8| {
            let on = (0..8).any(|pin| senses(pin, self.io[base + pin] & PORT_ISC));
            u32::from(on) << vector
        };
        let pit = self.io[reg::RTC_PITINTCTRL] & RTC_PI != 0;
        let ovf = tca && self.io[reg::TCA0_INTCTRL] & TCA_OVF != 0;
        port(reg::PORTA_PIN0CTRL, VECTOR_PORTA)
            | port(reg::PORTB_PIN0CTRL, VECTOR_PORTB)
            | port(reg::PORTC_PIN0CTRL, VECTOR_PORTC)
            | u32::from(pit) << VECTOR_RTC_PIT
            | u32::from(ovf) << VECTOR_TCA0_OVF
    }

    fn update_enabled(&mut self) {
        self.enabled = self.interrupts(
            |_, isc| matches!(isc, ISC_BOTHEDGES | ISC_RISING | ISC_FALLING | ISC_LEVEL),
            true,
        );
    }

    /// TCA0 stops, and only pins 2 and 6 are fully asynchronous (table 16-3).
    fn wakes_from_power_down(&self) -> u32 {
        self.interrupts(
            |pin, isc| match isc {
                ISC_BOTHEDGES | ISC_LEVEL => true,
                ISC_RISING | ISC_FALLING => pin == 2 || pin == 6,
                _ => false,
            },
            false,
        )
    }

    fn ready_for_power_down(&self) -> Result<(), String> {
        if self.enabled_high() {
            return Err("powers down with PB2 high, so the light's on and can't be \
                        regulated with TCA0 stopped"
                .into());
        }
        if self.tca.cmp0buf != 0 {
            return Err(format!(
                "powers down with CMP0BUF at {}, which would land on waking",
                self.tca.cmp0buf
            ));
        }
        // §20.3.6 and §16.3.5 don't say what a waveform output holds with
        // TCA0's clock stopped.
        if self.io[reg::TCA0_CTRLB] & TCA_CMP0EN != 0 {
            return Err(
                "powers down with TCA0.CTRLB.CMP0EN set, so PB0 is left to a stopped \
                 timer, at a level the datasheet doesn't give"
                    .into(),
            );
        }
        // Registers keep their contents through sleep (§11.2).
        if self.tca.cmp0 != 0 {
            return Err(format!(
                "powers down with CMP0 at {}, which the next turn-on runs a cycle of \
                 before BOTTOM brings its own duty in",
                self.tca.cmp0
            ));
        }
        if self.converting() {
            return Err(
                "powers down mid-conversion, which resumes on waking with an \
                        invalid result (section 30.3.5)"
                    .into(),
            );
        }
        if self.io[reg::ADC0_CTRLA] & ADC_ENABLE != 0 {
            return Err("powers down with ADC0 still enabled".into());
        }
        if self.io[reg::VREF_CTRLB] & VREF_ADC0REFEN != 0 {
            return Err(
                "powers down with VREF.CTRLB.ADC0REFEN set, forcing the reference on \
                        for as long as it sleeps"
                    .into(),
            );
        }
        if self.pit_busy > 0 {
            return Err(
                "powers down with RTC.PITSTATUS.CTRLBUSY still set, and the \
                        datasheet doesn't say the write finishes syncing with CLK_PER stopped"
                    .into(),
            );
        }
        Ok(())
    }

    fn write_pin_ctrl(&mut self, a: usize, value: u8) -> Result<(), String> {
        let pull_up = value & PORT_PULLUPEN != 0;
        if pull_up && a == reg::PORTB_PIN0CTRL + ENABLE_PIN {
            return Err("gives PB2 a pull-up, which could creep the boost converter on".into());
        }
        if a == reg::PORTA_PIN0CTRL + UPDI_PIN {
            return Err(format!(
                "writes {value:#04x} to {}: PA0 is UPDI, and the firmware leaves its \
                 settings alone",
                name(a)
            ));
        }
        if pull_up && a == reg::PORTA_PIN0CTRL + BATTERY_PIN {
            return Err(format!(
                "writes {value:#04x} to {}: PA1 is the battery divider, and a pull-up \
                 would skew it",
                name(a)
            ));
        }
        if a == reg::PORTA_PIN0CTRL + SWITCH_PIN && value & PORT_INVEN != 0 {
            return Err("sets INVEN on PA5, which the simulator doesn't model".into());
        }
        self.io[a] = value;
        self.update_enabled();
        Ok(())
    }

    fn write_portb(&mut self, a: usize, value: u8) -> Result<(), String> {
        let was = self.enabled_high();
        if a == reg::VPORTB_OUT && value & !ENABLE != 0 {
            return Err(format!(
                "writes {value:#04x} to VPORTB.OUT: only PB2 is driven by hand, since \
                 PB0's TCA0's"
            ));
        }
        self.io[a] = value;
        let high = self.enabled_high();
        if high && !was {
            let handler = self.enabled & 1 << VECTOR_TCA0_OVF != 0;
            if !handler && self.tca.cmp0 == 0 && self.tca.cmp0buf == 0 {
                return Err(
                    "raises PB2 with the duty at 0: the duty goes first, then the enable".into(),
                );
            }
        }
        if high != was {
            self.tca.zero_run = 0;
            self.tca.settled = 0;
        }
        Ok(())
    }

    fn tca_written(&mut self, low: usize, value: u16) -> Result<(), String> {
        match low {
            reg::TCA0_PERBUF | reg::TCA0_PER if value != PWM_TOP => Err(format!(
                "writes {value} to {}, but the PWM's frequency and every duty assume {PWM_TOP}",
                name(low)
            )),
            reg::TCA0_PERBUF => {
                self.tca.perbuf = value;
                self.tca.perbv = true;
                Ok(())
            }
            reg::TCA0_PER if self.tca_running() => Err(
                "writes TCA0.PER with the timer running, which applies at once and can \
                 wrap the counter (section 20.3.3.3): use PERBUF"
                    .into(),
            ),
            reg::TCA0_PER => {
                self.tca.per = value;
                Ok(())
            }
            reg::TCA0_CMP0 if self.enabled_high() => Err(
                "writes TCA0.CMP0 with PB2 high, which lands at once rather than at BOTTOM: \
                 that's only for turning off, once the enable's dropped"
                    .into(),
            ),
            reg::TCA0_CMP0 => {
                self.tca.cmp0 = value;
                Ok(())
            }
            reg::TCA0_CMP0BUF if self.enabled_high() && !self.in_handler => Err(
                "writes TCA0.CMP0BUF from the main loop with PB2 high: only the \
                 delta-sigma handler may, or the two race for TEMP"
                    .into(),
            ),
            reg::TCA0_CMP0BUF => {
                self.tca.cmp0buf = value;
                self.tca.cmp0bv = true;
                Ok(())
            }
            _ => Err(format!(
                "writes {}, which the simulator doesn't model",
                name(low)
            )),
        }
    }

    fn temp_undisturbed(&self, a: usize) -> Result<(), String> {
        let Some(low) = self.interrupted else {
            return Ok(());
        };
        if (reg::TCA0_CNT..TCA0_WIDE_END).contains(&a) {
            return Err(format!(
                "a handler touches {} with the main loop halfway through {}, and they \
                 share TCA0's TEMP byte, so one corrupts the other (section 8.5.6)",
                name(a & !1),
                name(low)
            ));
        }
        Ok(())
    }
}

fn plain(addr: usize) -> bool {
    let pin_ctrl = (reg::PORTC_PIN0CTRL..reg::PORTC_PIN0CTRL + 8).contains(&addr);
    pin_ctrl
        || (reg::GPIO_GPIOR0..=reg::GPIO_GPIOR3).contains(&addr)
        || matches!(
            addr,
            reg::VPORTB_DIR
                | reg::VPORTB_OUT
                | reg::VPORTC_DIR
                | reg::VPORTC_OUT
                | reg::CLKCTRL_MCLKCTRLB
                | reg::RTC_PITCTRLA
                | reg::RTC_PITINTCTRL
                | reg::SLPCTRL_CTRLA
                | reg::VREF_CTRLA
                | reg::VREF_CTRLB
                | reg::ADC0_CALIB
                | reg::ADC0_CTRLA
                | reg::ADC0_CTRLB
                | reg::ADC0_CTRLC
                | reg::ADC0_CTRLD
                | reg::ADC0_SAMPCTRL
                | reg::ADC0_MUXPOS
                | reg::WDT_CTRLA
                | reg::TCA0_CTRLA
                | reg::TCA0_CTRLB
                | reg::TCA0_INTCTRL
        )
}

fn vector_name(vector: u8) -> String {
    match vector {
        VECTOR_PORTA => "PORTA_PORT (vector 3)".into(),
        VECTOR_PORTB => "PORTB_PORT (vector 4)".into(),
        VECTOR_PORTC => "PORTC_PORT (vector 5)".into(),
        VECTOR_RTC_PIT => "RTC_PIT (vector 7)".into(),
        VECTOR_TCA0_OVF => "TCA0_OVF (vector 8)".into(),
        v => format!("vector {v}"),
    }
}

const ADC_SETUP: [usize; 9] = [
    reg::VREF_CTRLA,
    reg::VREF_CTRLB,
    reg::ADC0_CALIB,
    reg::ADC0_CTRLA,
    reg::ADC0_CTRLB,
    reg::ADC0_CTRLC,
    reg::ADC0_CTRLD,
    reg::ADC0_SAMPCTRL,
    reg::ADC0_MUXPOS,
];

fn fuse_or_lock(addr: usize) -> bool {
    let start = usize::from(map::FUSES);
    (start..start + map::FUSES_SIZE).contains(&addr)
}

fn name(addr: usize) -> String {
    if let Some(fuse) = addr
        .checked_sub(usize::from(map::FUSES))
        .and_then(|offset| FUSE_NAMES.get(offset))
        .filter(|name| !name.is_empty())
    {
        return format!("FUSE.{fuse}");
    }
    let known = [
        (reg::RSTCTRL_RSTFR, "RSTCTRL.RSTFR"),
        (reg::WDT_CTRLA, "WDT.CTRLA"),
        (reg::WDT_STATUS, "WDT.STATUS"),
        (reg::VREF_CTRLA, "VREF.CTRLA"),
        (reg::VREF_CTRLB, "VREF.CTRLB"),
        (reg::CRCSCAN_CTRLA, "CRCSCAN.CTRLA"),
        (reg::CRCSCAN_CTRLB, "CRCSCAN.CTRLB"),
        (reg::CRCSCAN_STATUS, "CRCSCAN.STATUS"),
        (reg::ADC0_CALIB, "ADC0.CALIB"),
        (reg::ADC0_CTRLA, "ADC0.CTRLA"),
        (reg::ADC0_CTRLB, "ADC0.CTRLB"),
        (reg::ADC0_CTRLC, "ADC0.CTRLC"),
        (reg::ADC0_CTRLD, "ADC0.CTRLD"),
        (reg::ADC0_SAMPCTRL, "ADC0.SAMPCTRL"),
        (reg::ADC0_MUXPOS, "ADC0.MUXPOS"),
        (reg::ADC0_COMMAND, "ADC0.COMMAND"),
        (reg::ADC0_INTFLAGS, "ADC0.INTFLAGS"),
        (reg::ADC0_RES, "ADC0.RESL"),
        (reg::ADC0_RES + 1, "ADC0.RESH"),
        (reg::SIGROW_TEMPSENSE0, "SIGROW.TEMPSENSE0"),
        (reg::SIGROW_TEMPSENSE1, "SIGROW.TEMPSENSE1"),
        (reg::CPU_CCP, "CPU.CCP"),
        (reg::VPORTA_IN, "VPORTA.IN"),
        (reg::VPORTB_DIR, "VPORTB.DIR"),
        (reg::VPORTB_OUT, "VPORTB.OUT"),
        (reg::VPORTC_DIR, "VPORTC.DIR"),
        (reg::VPORTC_OUT, "VPORTC.OUT"),
        (reg::SLPCTRL_CTRLA, "SLPCTRL.CTRLA"),
        (reg::CLKCTRL_MCLKCTRLB, "CLKCTRL.MCLKCTRLB"),
        (reg::RTC_PITCTRLA, "RTC.PITCTRLA"),
        (reg::RTC_PITSTATUS, "RTC.PITSTATUS"),
        (reg::RTC_PITINTCTRL, "RTC.PITINTCTRL"),
        (reg::RTC_PITINTFLAGS, "RTC.PITINTFLAGS"),
        (reg::PORTA_INTFLAGS, "PORTA.INTFLAGS"),
        (reg::PORTA_PIN0CTRL, "PORTA.PIN0CTRL"),
        (reg::PORTA_PIN0CTRL + 1, "PORTA.PIN1CTRL"),
        (reg::TCA0_CTRLA, "TCA0.CTRLA"),
        (reg::TCA0_CTRLB, "TCA0.CTRLB"),
        (reg::TCA0_INTCTRL, "TCA0.INTCTRL"),
        (reg::TCA0_INTFLAGS, "TCA0.INTFLAGS"),
        (DSM_DUTY, "GPIOR1 (the delta-sigma's duty)"),
        (DSM_FRACTION, "GPIOR2 (the delta-sigma's fraction)"),
        (DSM_SUM, "GPIOR3 (the delta-sigma's sum)"),
        (reg::TCA0_CNT, "TCA0.CNT"),
        (reg::TCA0_PER, "TCA0.PER"),
        (reg::TCA0_CMP0, "TCA0.CMP0"),
        (reg::TCA0_PERBUF, "TCA0.PERBUF"),
        (reg::TCA0_CMP0BUF, "TCA0.CMP0BUF"),
    ];
    known
        .iter()
        .find(|&&(a, _)| a == addr)
        .map_or_else(|| format!("{addr:#06x}"), |(_, n)| (*n).to_string())
}

impl Bus for Chip {
    fn fetch(&self, pc: u32) -> Option<u16> {
        word(&self.flash, pc as usize * 2)
    }

    fn wdr(&mut self) -> Result<(), String> {
        if self.wdt_open() == 0 {
            return Ok(());
        }
        if self.wdt.busy > 0 {
            return Err("wdr before the WDT.CTRLA write has synced".into());
        }
        let clocks = self.wdt.ticks * WDT_CLOCKS_PER_TICK;
        let closed = self.wdt_closed();
        if closed > 0 && clocks <= closed {
            return Err(format!(
                "wdr {} ticks ({clocks} WDT clocks) after the last, in or on the edge of \
                 the {closed}-clock closed window, so the chip would have reset",
                self.wdt.ticks
            ));
        }
        self.wdt.ticks = 0;
        self.wdt.windowed = self.io[reg::WDT_CTRLA] & WDT_WINDOW != 0;
        Ok(())
    }

    /// Lowest vector first, the default static priority (§13.3.2.4).
    fn interrupt(&self) -> Option<u8> {
        let pending = u32::from(self.porta_flags != 0) << VECTOR_PORTA
            | u32::from(self.pit_flag) << VECTOR_RTC_PIT
            | u32::from(self.tca.ovf) << VECTOR_TCA0_OVF;
        let forced = self.forced.map_or(0, |vector| 1 << vector);
        let requests = pending & self.enabled | forced;
        (requests != 0).then(|| requests.trailing_zeros() as u8)
    }

    /// Four instructions after the key, left pending (§8.7.1, §8.5.7.2). That
    /// the store or `sleep` ending the protected change ends this too is
    /// likely, not stated.
    fn holds_interrupts(&self) -> bool {
        self.ccp_hold && self.unlocked_until.is_some_and(|until| self.now <= until)
    }

    /// With none enabled at all, only a reset wakes it (§11.3.1).
    fn sleep(&mut self) -> Result<bool, String> {
        self.ccp_hold = false;
        self.tick_polled = true;
        let ctrla = self.io[reg::SLPCTRL_CTRLA];
        if ctrla & SLPCTRL_SEN == 0 {
            return Err(
                "sleeps with SLPCTRL.SEN clear, which the chip treats as a nop, \
                        so the firmware would spin instead"
                    .into(),
            );
        }
        let mut wakes = if self.powering_down() {
            self.ready_for_power_down()?;
            self.power_downs += 1;
            self.wakes_from_power_down()
        } else if ctrla == SLEEP_IDLE | SLPCTRL_SEN {
            self.enabled
        } else {
            return Err(format!(
                "sleeps with SLPCTRL.CTRLA {ctrla:#04x}, but the simulator only models \
                 idle and power-down"
            ));
        };
        if self.io[reg::RTC_PITCTRLA] & RTC_PITEN == 0 {
            wakes &= !(1 << VECTOR_RTC_PIT);
        }
        if wakes == 0 && self.enabled != 0 {
            return Err("sleeps with no interrupt enabled that could wake it".into());
        }
        self.down = wakes == 0;
        Ok(wakes != 0)
    }

    /// TCA0 stops in power-down (§20.3.6 only promises idle).
    fn idle(&mut self) -> Result<(), String> {
        if self.powering_down() {
            self.tca.next_bottom += self.next_pit - self.now;
            self.plan_next_event();
            return self.advance_to(self.next_pit);
        }
        self.advance_to(self.next_event)
    }

    fn read(&mut self, addr: u16) -> Result<u8, String> {
        let a = usize::from(addr);
        self.temp_undisturbed(a)?;
        if SRAM.contains(&addr) {
            return Ok(self.sram[usize::from(addr - SRAM.start)]);
        }
        if (MAPPED..MAPPED + FLASH_SIZE as u16).contains(&addr) {
            return Ok(*self.flash.get(usize::from(addr - MAPPED)).unwrap_or(&0xFF));
        }
        match a {
            // Every ISC but INPUT_DISABLE leaves the input buffer on (§16.3.2.1,
            // §16.5.11). Whether the board has its own pull-up on PA5 is
            // unconfirmed, so it's taken that it hasn't. PA1's bit stops
            // following the pin with its buffer off (§16.5.11's note), and
            // what it then holds is unknown, so it reads 1 here.
            reg::VPORTA_IN => {
                let pa1 = self.io[reg::PORTA_PIN0CTRL + BATTERY_PIN];
                if pa1 & PORT_ISC != ISC_INPUT_DISABLE {
                    return Err(
                        "reads VPORTA.IN with PA1's digital input buffer on, which reads the \
                         battery divider as a logic level"
                            .into(),
                    );
                }
                let ctrl = self.switch_ctrl();
                if ctrl & PORT_ISC == ISC_INPUT_DISABLE {
                    return Err(
                        "reads PA5 with its input buffer off, so PORTA.IN never changes".into(),
                    );
                }
                if ctrl & PORT_PULLUPEN == 0 {
                    return Err(
                        "reads PA5 without its pull-up, and nothing else is known to pull \
                         a released switch high"
                            .into(),
                    );
                }
                Ok(if self.pressed { !SWITCH } else { 0xFF })
            }
            reg::RTC_PITSTATUS => {
                let busy = self.pit_busy > 0;
                self.pit_busy = self.pit_busy.saturating_sub(1);
                Ok(if busy { RTC_CTRLBUSY } else { 0 })
            }
            reg::RTC_PITINTFLAGS => {
                // Finding it up is a wait that's ending, not one starting.
                self.tick_polled |= !self.pit_flag;
                Ok(if self.pit_flag { RTC_PI } else { 0 })
            }
            reg::VPORTA_INTFLAGS | reg::PORTA_INTFLAGS => Ok(self.porta_flags),
            reg::TCA0_INTFLAGS => Ok(if self.tca.ovf { TCA_OVF } else { 0 }),
            reg::WDT_STATUS => {
                let busy = self.wdt.busy > 0;
                self.wdt.busy = self.wdt.busy.saturating_sub(1);
                Ok(if busy { WDT_SYNCBUSY } else { 0 })
            }
            reg::CRCSCAN_STATUS => Ok(self.crc_status()),
            reg::RSTCTRL_RSTFR => Ok(self.rstfr),
            reg::ADC0_INTFLAGS => {
                let ready = self.adc.ready_at.is_some_and(|at| self.now >= at);
                Ok(if ready && !self.world.scene.adc_dead {
                    ADC_RESRDY
                } else {
                    0
                })
            }
            reg::ADC0_RES => {
                if self.adc.ready_at.is_none_or(|at| self.now < at) {
                    return Err("reads ADC0.RES with no result ready".into());
                }
                self.adc.ready_at = None;
                let [lo, hi] = self.adc.result.to_le_bytes();
                self.adc.temp = Some(hi);
                Ok(lo)
            }
            _ if a == reg::ADC0_RES + 1 => self.adc.temp.take().ok_or_else(|| {
                "reads ADC0.RES's high byte without its low byte just before, so it gets \
                 whatever ADC0.TEMP held"
                    .into()
            }),
            reg::SIGROW_TEMPSENSE0 => Ok(CALIBRATION.gain),
            reg::SIGROW_TEMPSENSE1 => Ok(CALIBRATION.offset as u8),
            _ if self.world.fuse(a).is_some() => {
                self.boot.fuses_read.get_or_insert(self.now);
                Ok(self.world.fuse(a).unwrap_or(0xFF))
            }
            _ if plain(a) || (reg::PORTA_PIN0CTRL..reg::PORTA_PIN0CTRL + 8).contains(&a) => {
                Ok(self.io[a])
            }
            _ if (reg::PORTB_PIN0CTRL..reg::PORTB_PIN0CTRL + 8).contains(&a) => Ok(self.io[a]),
            _ => Err(format!(
                "reads {}, which the simulator doesn't model",
                name(a)
            )),
        }
    }

    fn write(&mut self, addr: u16, value: u8) -> Result<(), String> {
        let a = usize::from(addr);
        self.temp_undisturbed(a)?;
        self.ccp_hold = false;
        if SRAM.contains(&addr) {
            self.sram[usize::from(addr - SRAM.start)] = value;
            return Ok(());
        }
        self.register_writes += 1;
        self.boot.write(self.now, a, value);
        match a {
            reg::CPU_CCP => {
                if value != CCP_IOREG {
                    return Err(format!(
                        "writes {value:#04x} to CPU.CCP, which isn't the IOREG key"
                    ));
                }
                self.unlocked_until = Some(self.now + 4);
                self.ccp_hold = true;
            }
            reg::CLKCTRL_MCLKCTRLB => {
                self.take_protected_write(a)?;
                if value != CLOCK_10MHZ {
                    return Err(format!(
                        "writes {value:#04x} to CLKCTRL.MCLKCTRLB: the firmware runs at \
                         10 MHz, and the PWM's frequency depends on it"
                    ));
                }
                self.io[a] = value;
            }
            reg::VPORTB_DIR | reg::VPORTB_OUT => self.write_portb(a, value)?,
            reg::TCA0_CTRLA => {
                if value != TCA_RUNNING {
                    return Err(format!(
                        "writes {value:#04x} to TCA0.CTRLA: the period and the duty \
                         assume the timer runs undivided, and it never stops"
                    ));
                }
                self.io[a] = value;
                // Unconfirmed: a full first cycle at whatever PER is.
                self.tca.next_bottom = self.now + 2 * u64::from(self.tca.per.max(1));
                self.plan_next_event();
            }
            reg::TCA0_CTRLB => {
                if value & !TCA_CMP0EN != TCA_DSBOTTOM {
                    return Err(format!(
                        "writes {value:#04x} to TCA0.CTRLB: the period and the duty assume \
                         dual-slope at BOTTOM on WO0"
                    ));
                }
                self.io[a] = value;
            }
            reg::TCA0_INTCTRL
                if value & TCA_OVF != 0 && self.io[a] & TCA_OVF == 0 && self.io[DSM_SUM] != 0 =>
            {
                return Err(format!(
                    "turns TCA0's overflow interrupt on with the delta-sigma's sum at {}, \
                     left over from the last time the light was on",
                    self.io[DSM_SUM]
                ));
            }
            // The request is the flag and the enable together, and BOTTOM sets
            // the flag with the interrupt off too (§20.3.5). One that went up
            // between a clear and this write is a real BOTTOM, and fine.
            reg::TCA0_INTCTRL
                if value & TCA_OVF != 0
                    && self.io[a] & TCA_OVF == 0
                    && self.tca.ovf
                    && self.now - self.tca.ovf_up_at >= PWM_CYCLE =>
            {
                return Err(
                    "turns TCA0's overflow interrupt on with INTFLAGS.OVF still up from \
                     before, so the handler runs at once rather than at the next BOTTOM \
                     (section 20.3.5)"
                        .into(),
                );
            }
            reg::TCA0_INTCTRL | reg::RTC_PITINTCTRL => {
                self.io[a] = value;
                self.update_enabled();
            }
            DSM_DUTY | DSM_FRACTION | DSM_SUM
                if !self.in_handler
                    && self.interrupts_on
                    && self.enabled & 1 << VECTOR_TCA0_OVF != 0 =>
            {
                return Err(format!(
                    "writes {} from the main loop with interrupts on and TCA0's overflow \
                     enabled, so the handler can catch the delta-sigma half changed",
                    name(a)
                ));
            }
            reg::TCA0_INTFLAGS => {
                if value & TCA_OVF != 0 {
                    self.tca.ovf = false;
                }
            }
            reg::RTC_PITCTRLA => {
                if self.pit_busy > 0 {
                    return Err(format!(
                        "writes {} while RTC.PITSTATUS.CTRLBUSY is set, and the chip drops \
                         that write",
                        name(a)
                    ));
                }
                self.io[a] = value;
                self.pit_busy = PIT_SYNC_READS;
            }
            reg::RTC_PITINTFLAGS => {
                if value & RTC_PI != 0 {
                    self.pit_flag = false;
                }
            }
            reg::VPORTA_INTFLAGS | reg::PORTA_INTFLAGS => self.porta_flags &= !value,
            _ if (reg::PORTA_PIN0CTRL..reg::PORTA_PIN0CTRL + 8).contains(&a)
                || (reg::PORTB_PIN0CTRL..reg::PORTB_PIN0CTRL + 8).contains(&a)
                || (reg::PORTC_PIN0CTRL..reg::PORTC_PIN0CTRL + 8).contains(&a) =>
            {
                self.write_pin_ctrl(a, value)?;
            }
            reg::TCA0_PER | reg::TCA0_PERBUF | reg::TCA0_CMP0 | reg::TCA0_CMP0BUF => {
                if let Some((pending, _)) = self.tca.temp {
                    return Err(format!(
                        "writes the low byte of {} while {}'s is still waiting for its high byte",
                        name(a),
                        name(pending)
                    ));
                }
                self.tca.temp = Some((a, value));
            }
            _ if (reg::TCA0_CNT..TCA0_WIDE_END).contains(&a) && a & 1 != 0 => {
                match self.tca.temp.take() {
                    Some((low, lo)) if low == a - 1 => {
                        self.tca_written(low, u16::from_le_bytes([lo, value]))?;
                    }
                    _ => {
                        return Err(format!(
                            "writes the high byte of {} without its low byte just before, \
                             so the chip pairs it with whatever TEMP held",
                            name(a - 1)
                        ));
                    }
                }
            }
            reg::WDT_CTRLA => {
                self.take_protected_write(a)?;
                if self.wdt.busy > 0 {
                    return Err("writes WDT.CTRLA while WDT.STATUS.SYNCBUSY is set, \
                                which the datasheet doesn't allow (section 19.3.6)"
                        .into());
                }
                if self.wdt_open() > 0 {
                    return Err(format!(
                        "writes {value:#04x} to WDT.CTRLA with the watchdog already on, \
                         which the simulator doesn't model, and off isn't allowed"
                    ));
                }
                if value & WDT_PERIOD > WDT_PERIOD_MAX || (value & WDT_WINDOW) >> 4 > WDT_PERIOD_MAX
                {
                    return Err(format!(
                        "writes {value:#04x} to WDT.CTRLA: a reserved WDT period or window"
                    ));
                }
                self.io[a] = value;
                self.wdt = Wdt {
                    busy: WDT_SYNC_READS,
                    ..Wdt::default()
                };
            }
            reg::WDT_STATUS => {
                return Err(if value & WDT_LOCK != 0 {
                    "sets WDT.STATUS.LOCK, which only debug mode can clear".into()
                } else {
                    "writes WDT.STATUS, whose only writable bit is LOCK".into()
                });
            }
            reg::RSTCTRL_RSTFR => self.rstfr &= !value,
            reg::CRCSCAN_CTRLA => {
                if value != CRCSCAN_ENABLE {
                    return Err(format!(
                        "writes {value:#04x} to CRCSCAN.CTRLA; the simulator only models \
                         ENABLE, with STATUS polled (NMIEN can't be undone without a reset)"
                    ));
                }
                self.start_crc()?;
                self.io[a] = value;
            }
            reg::CRCSCAN_CTRLB => self.io[a] = value,
            reg::ADC0_COMMAND => {
                if value & ADC_STCONV != 0 {
                    self.start_conversion()?;
                }
            }
            reg::ADC0_INTFLAGS => {
                // A conversion still running sets it again when it finishes.
                if value & ADC_RESRDY != 0 && !self.converting() {
                    self.adc.ready_at = None;
                }
            }
            _ if ADC_SETUP.contains(&a) && self.converting() => {
                return Err(format!("writes {} mid-conversion", name(a)));
            }
            _ if fuse_or_lock(a) => {
                return Err(format!(
                    "writes {value:#04x} to {}: the firmware never writes the fuses",
                    name(a)
                ));
            }
            _ if plain(a) => {
                if a == reg::ADC0_CTRLA && value & ADC_ENABLE == 0 {
                    // The next conversion catches the ADC starting up.
                    self.adc.last_setup = None;
                }
                self.io[a] = value;
            }
            _ => {
                return Err(format!(
                    "writes {value:#04x} to {}, which the simulator doesn't model",
                    name(a)
                ));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ts26::ui::Aux;

    const PA1_CTRL: usize = reg::PORTA_PIN0CTRL + BATTERY_PIN;

    fn adc_ready(muxpos: u8) -> Chip {
        let mut chip = Chip::new(Vec::new());
        for (a, value) in [
            (PA1_CTRL, ISC_INPUT_DISABLE),
            (reg::ADC0_CALIB, ADC_DUTY25),
            (reg::ADC0_CTRLB, ADC_SAMPNUM_ACC4),
            (reg::ADC0_CTRLC, ADC_SAMPCAP | ADC_REFSEL_INTREF | 0x03),
            (reg::ADC0_CTRLD, 0x40),
            (reg::ADC0_SAMPCTRL, 22),
            (reg::VREF_CTRLB, VREF_ADC0REFEN),
            (reg::VREF_CTRLA, VREF_1V1),
            (reg::ADC0_MUXPOS, muxpos),
            (reg::ADC0_CTRLA, ADC_ENABLE),
        ] {
            chip.write(a as u16, value).unwrap();
        }
        chip
    }

    fn convert(chip: &mut Chip) -> u16 {
        chip.write(reg::ADC0_COMMAND as u16, ADC_STCONV).unwrap();
        chip.advance_to(chip.now + CONVERSION_INSTRUCTIONS).unwrap();
        assert_eq!(chip.read(reg::ADC0_INTFLAGS as u16), Ok(ADC_RESRDY));
        let lo = chip.read(reg::ADC0_RES as u16).unwrap();
        let hi = chip.read(reg::ADC0_RES as u16 + 1).unwrap();
        u16::from_le_bytes([lo, hi])
    }

    #[test]
    fn the_first_conversion_after_a_change_is_junk_then_it_reads_the_world() {
        for (muxpos, channel) in [
            (ADC_MUXPOS_AIN1, Channel::Voltage),
            (ADC_MUXPOS_TEMPSENSE, Channel::Temperature),
        ] {
            let mut chip = adc_ready(muxpos);
            assert_eq!(convert(&mut chip), UNSETTLED);
            assert_eq!(convert(&mut chip), World::NORMAL.reading(channel));
        }
    }

    /// §30.3.1.1.
    #[test]
    fn converting_ain1_wants_pa1s_input_buffer_off_and_nothing_else() {
        let start = |pa1: u8, muxpos: u8| {
            let mut chip = adc_ready(muxpos);
            chip.write(PA1_CTRL as u16, pa1).unwrap();
            chip.write(reg::ADC0_COMMAND as u16, ADC_STCONV)
        };
        assert_eq!(start(ISC_INPUT_DISABLE, ADC_MUXPOS_AIN1), Ok(()));
        for pa1 in [ISC_INTDISABLE, ISC_INPUT_DISABLE | PORT_INVEN] {
            let err = start(pa1, ADC_MUXPOS_AIN1).unwrap_err();
            assert!(err.contains("PORTA.PIN1CTRL"), "{err}");
        }
        assert_eq!(start(ISC_INTDISABLE, ADC_MUXPOS_TEMPSENSE), Ok(()));
    }

    #[test]
    fn a_result_isnt_there_until_the_conversion_ends() {
        let mut chip = adc_ready(ADC_MUXPOS_AIN1);
        chip.write(reg::ADC0_COMMAND as u16, ADC_STCONV).unwrap();
        assert_eq!(chip.read(reg::ADC0_INTFLAGS as u16), Ok(0));
        assert!(
            chip.read(reg::ADC0_RES as u16)
                .unwrap_err()
                .contains("no result")
        );
        let err = chip.write(reg::ADC0_MUXPOS as u16, 0).unwrap_err();
        assert!(err.contains("mid-conversion"), "{err}");
    }

    #[test]
    fn power_down_wants_the_adc_and_its_reference_off() {
        let mut chip = adc_ready(ADC_MUXPOS_AIN1);
        chip.pit_busy = 0;
        let err = chip.ready_for_power_down().unwrap_err();
        assert!(err.contains("ADC0 still enabled"), "{err}");
        chip.write(reg::ADC0_CTRLA as u16, 0).unwrap();
        let err = chip.ready_for_power_down().unwrap_err();
        assert!(err.contains("ADC0REFEN"), "{err}");
        chip.write(reg::VREF_CTRLB as u16, 0).unwrap();
        chip.ready_for_power_down().unwrap();
    }

    /// 32 WDT clocks closed and 32 open, two ticks each.
    fn watched() -> Chip {
        let mut chip = Chip::new(Vec::new());
        chip.write(reg::CPU_CCP as u16, CCP_IOREG).unwrap();
        chip.write(reg::WDT_CTRLA as u16, 0x33).unwrap();
        chip.wdt.busy = 0;
        chip
    }

    fn ticks(chip: &mut Chip, n: u64) -> Result<(), String> {
        chip.advance_to(chip.now + n * TICK_CYCLES)
    }

    #[test]
    fn a_feed_in_the_open_window_is_fine_and_a_closed_one_bites() {
        let mut chip = watched();
        chip.wdr().unwrap();
        ticks(&mut chip, 3).unwrap();
        chip.wdr().unwrap();
        ticks(&mut chip, 1).unwrap();
        assert!(chip.wdr().unwrap_err().contains("closed window"));
    }

    #[test]
    fn going_without_a_feed_bites() {
        let mut chip = watched();
        let err = ticks(&mut chip, 2).unwrap_err();
        assert!(err.contains("without wdr"), "{err}");
    }

    #[test]
    fn the_watchdog_wont_be_turned_off_or_fed_before_it_syncs() {
        let mut chip = watched();
        chip.write(reg::CPU_CCP as u16, CCP_IOREG).unwrap();
        let err = chip.write(reg::WDT_CTRLA as u16, 0).unwrap_err();
        assert!(err.contains("already on"), "{err}");
        let mut chip = Chip::new(Vec::new());
        chip.write(reg::CPU_CCP as u16, CCP_IOREG).unwrap();
        chip.write(reg::WDT_CTRLA as u16, 0x33).unwrap();
        assert!(chip.wdr().unwrap_err().contains("synced"));
    }

    #[test]
    fn the_scan_passes_a_signed_image_and_fails_a_changed_one() {
        let signed = crc::sign(&[0x12, 0x34], &stamp::TEST).unwrap();
        let mut bad = signed.clone();
        bad[0] ^= 1;
        for (image, want) in [(signed, CRCSCAN_OK), (bad, 0)] {
            let mut chip = Chip::new(image);
            chip.write(reg::CRCSCAN_CTRLA as u16, CRCSCAN_ENABLE)
                .unwrap();
            assert_eq!(chip.read(reg::CRCSCAN_STATUS as u16), Ok(CRCSCAN_OK));
            chip.advance_to(chip.now + CRC_START_INSTRUCTIONS + 1)
                .unwrap();
            assert_eq!(chip.read(reg::CRCSCAN_STATUS as u16), Ok(CRCSCAN_BUSY));
            assert_eq!(chip.read(reg::CRCSCAN_STATUS as u16), Ok(CRCSCAN_BUSY));
            assert_eq!(chip.read(reg::CRCSCAN_STATUS as u16), Ok(want));
        }
    }

    #[test]
    fn fuses_read_back_and_never_take_a_write() {
        let mut chip = Chip::new(Vec::new());
        let osccfg = ts26::regs::addr::FUSE_OSCCFG as u16;
        assert_eq!(chip.read(osccfg), Ok(0x02));
        assert!(
            chip.write(osccfg, 0x01)
                .unwrap_err()
                .contains("never writes")
        );
    }

    #[test]
    fn the_reset_flags_clear_by_writing_a_one() {
        let mut chip = Chip::new(Vec::new());
        assert_eq!(chip.read(reg::RSTCTRL_RSTFR as u16), Ok(RSTCTRL_PORF));
        chip.write(reg::RSTCTRL_RSTFR as u16, RSTCTRL_PORF).unwrap();
        assert_eq!(chip.read(reg::RSTCTRL_RSTFR as u16), Ok(0));
    }

    fn running() -> Chip {
        let mut chip = Chip::new(Vec::new());
        wide(&mut chip, reg::TCA0_PERBUF, PWM_TOP).unwrap();
        chip.write(reg::TCA0_CTRLB as u16, TCA_DSBOTTOM | TCA_CMP0EN)
            .unwrap();
        chip.write(reg::TCA0_CTRLA as u16, TCA_RUNNING).unwrap();
        chip.write(reg::VPORTB_DIR as u16, ENABLE | 1).unwrap();
        let first = chip.tca.next_bottom;
        chip.advance_to(first).unwrap();
        chip
    }

    fn overflow_on(chip: &mut Chip) {
        chip.write(reg::TCA0_INTFLAGS as u16, TCA_OVF).unwrap();
        chip.write(reg::TCA0_INTCTRL as u16, TCA_OVF).unwrap();
    }

    fn wide(chip: &mut Chip, low: usize, value: u16) -> Result<(), String> {
        let [lo, hi] = value.to_le_bytes();
        chip.write(low as u16, lo)?;
        chip.write(low as u16 + 1, hi)
    }

    fn cycles(chip: &mut Chip, n: u64) -> Result<(), String> {
        chip.advance_to(chip.now + n * PWM_CYCLE)
    }

    #[test]
    fn a_pwm_cycle_is_510_clocks_at_19_6_khz() {
        let mut chip = running();
        assert_eq!(chip.tca.per, PWM_TOP);
        let from = chip.tca.next_bottom;
        chip.tca.ovf = false;
        chip.advance_to(from - 1).unwrap();
        assert!(!chip.tca.ovf);
        chip.elapse().unwrap();
        assert!(chip.tca.ovf);
        assert_eq!(chip.tca.next_bottom - from, 510);
        assert_eq!(10_000_000 / PWM_CYCLE, 19_607);
    }

    #[test]
    fn a_compare_lands_at_bottom_not_when_written() {
        let mut chip = running();
        wide(&mut chip, reg::TCA0_CMP0BUF, 10).unwrap();
        assert_eq!(chip.tca.cmp0, 0);
        cycles(&mut chip, 1).unwrap();
        assert_eq!(chip.tca.cmp0, 10);
    }

    #[test]
    fn the_enable_goes_after_the_duty() {
        let mut chip = running();
        let err = chip.write(reg::VPORTB_OUT as u16, ENABLE).unwrap_err();
        assert!(err.contains("the duty goes first"), "{err}");
        let mut chip = running();
        wide(&mut chip, reg::TCA0_CMP0BUF, 10).unwrap();
        chip.write(reg::VPORTB_OUT as u16, ENABLE).unwrap();
        assert!(chip.enabled_high());
    }

    #[test]
    fn a_dsm_gap_is_fine_but_a_longer_zero_duty_is_not() {
        let mut chip = running();
        overflow_on(&mut chip);
        chip.write(reg::VPORTB_OUT as u16, ENABLE).unwrap();
        cycles(&mut chip, u64::from(ZERO_DUTY_CYCLES)).unwrap();
        let err = cycles(&mut chip, 1).unwrap_err();
        assert!(err.contains("duty at 0"), "{err}");
    }

    #[test]
    fn dropping_the_enable_ends_a_zero_duty_run() {
        let mut chip = running();
        overflow_on(&mut chip);
        chip.write(reg::VPORTB_OUT as u16, ENABLE).unwrap();
        cycles(&mut chip, u64::from(ZERO_DUTY_CYCLES)).unwrap();
        chip.write(reg::VPORTB_OUT as u16, 0).unwrap();
        cycles(&mut chip, 100).unwrap();
    }

    #[test]
    fn only_a_handler_writes_the_compare_with_the_light_on() {
        let mut chip = running();
        wide(&mut chip, reg::TCA0_CMP0BUF, 10).unwrap();
        chip.write(reg::VPORTB_OUT as u16, ENABLE).unwrap();
        let err = wide(&mut chip, reg::TCA0_CMP0BUF, 11).unwrap_err();
        assert!(err.contains("only the delta-sigma handler"), "{err}");
        chip.tca.temp = None;
        chip.in_handler = true;
        wide(&mut chip, reg::TCA0_CMP0BUF, 11).unwrap();
    }

    #[test]
    fn a_handler_cant_touch_tca0_with_the_main_loop_mid_access() {
        let mut chip = running();
        chip.write(reg::TCA0_CMP0BUF as u16, 10).unwrap();
        chip.interrupted = chip.tca.temp.map(|(low, _)| low);
        chip.in_handler = true;
        let err = chip.write(reg::TCA0_CMP0BUF as u16, 11).unwrap_err();
        assert!(err.contains("TEMP"), "{err}");
        chip.write(reg::GPIO_GPIOR0 as u16, 1).unwrap();
    }

    const SLOT_13: u32 = 13 * VECTOR_WORDS * 2;

    /// `sei`, then `nop`s up to vector 13's slot, which writes CMP0BUF with
    /// r1, low byte then high, with `halt` at `halt`.
    fn forced(halt: u32) -> Sim {
        let sts = |a: usize| [0x9210, a as u16];
        let mut words = vec![0x9478u16];
        words.resize(SLOT_13 as usize / 2, 0);
        words.extend(sts(reg::TCA0_CMP0BUF));
        words.extend(sts(reg::TCA0_CMP0BUF + 1));
        words.extend([0; 4]);
        let image = words.into_iter().flat_map(u16::to_le_bytes).collect();
        let ends = Ends {
            unhandled: SLOT_13,
            halt,
        };
        Sim::new(image, SRAM.start, ends).unwrap()
    }

    #[test]
    fn a_forced_interrupt_waits_for_sreg_i_and_the_instruction_after_sei() {
        let mut sim = forced(0x200);
        sim.chip.force_interrupt(13);
        sim.step().unwrap(); // sei
        assert_eq!(sim.cpu.handling, None, "SREG.I was clear");
        sim.step().unwrap(); // the nop after it
        assert_eq!(
            sim.cpu.handling, None,
            "the instruction after sei runs first"
        );
        let sp = sim.cpu.sp;
        sim.step().unwrap();
        assert_eq!(sim.cpu.handling, Some(13));
        assert_eq!(sim.cpu.pc * 2, SLOT_13);
        assert_eq!(sim.cpu.sp, sp - 2, "pushed PC");
        assert!(sim.cpu.flag(I), "left SREG.I alone");
        assert_eq!((sim.forced, sim.chip.forced), (Some(13), None));
        assert!(!sim.abandoned);
    }

    #[test]
    fn a_handler_in_halt_may_overwrite_temp_and_no_other_may() {
        let run = |halt: u32| {
            let mut sim = forced(halt);
            sim.chip.tca.temp = Some((reg::TCA0_CMP0, 0x12));
            sim.chip.force_interrupt(13);
            for _ in 0..3 {
                sim.step()?;
            }
            assert_eq!(sim.cpu.handling, Some(13));
            for _ in 0..2 {
                sim.step()?;
            }
            Ok::<_, String>(sim)
        };
        let err = run(0x200).err().unwrap();
        assert!(err.contains("TEMP"), "{err}");
        let sim = run(SLOT_13).unwrap();
        assert!(sim.abandoned);
        assert_eq!(sim.chip.tca.cmp0buf, 0);
        assert!(sim.chip.tca.cmp0bv);
    }

    #[test]
    fn the_high_byte_needs_its_low_byte_first() {
        let mut chip = running();
        let err = chip.write(reg::TCA0_CMP0BUF as u16 + 1, 0).unwrap_err();
        assert!(err.contains("without its low byte"), "{err}");
    }

    #[test]
    fn a_compare_over_per_is_refused() {
        let mut chip = running();
        wide(&mut chip, reg::TCA0_CMP0BUF, PWM_TOP + 1).unwrap();
        let err = cycles(&mut chip, 1).unwrap_err();
        assert!(err.contains("holds PB0 high"), "{err}");
    }

    #[test]
    fn the_dangerous_pins_never_get_a_pull_up() {
        let mut chip = Chip::new(Vec::new());
        for a in [
            reg::PORTB_PIN0CTRL + ENABLE_PIN,
            reg::PORTA_PIN0CTRL,
            reg::PORTA_PIN0CTRL + 1,
        ] {
            assert!(chip.write(a as u16, PORT_PULLUPEN).is_err(), "{a:#x}");
        }
        chip.write(reg::PORTB_PIN0CTRL as u16 + 3, PORT_PULLUPEN)
            .unwrap();
    }

    #[test]
    fn pa0s_settings_are_never_written_and_pa1s_take_all_but_a_pull_up() {
        let mut chip = Chip::new(Vec::new());
        for value in [0, ISC_INPUT_DISABLE] {
            let err = chip.write(reg::PORTA_PIN0CTRL as u16, value).unwrap_err();
            assert!(err.contains("UPDI"), "{err}");
        }
        chip.write(PA1_CTRL as u16, ISC_INPUT_DISABLE).unwrap();
        let err = chip
            .write(PA1_CTRL as u16, ISC_INPUT_DISABLE | PORT_PULLUPEN)
            .unwrap_err();
        assert!(err.contains("skew"), "{err}");
    }

    /// `out a, r`, for an I/O address.
    fn out(a: usize, r: u8) -> u16 {
        0xB800 | (a as u16 & 0x30) << 5 | u16::from(r) << 4 | a as u16 & 0x0F
    }

    #[test]
    fn found_lit_wants_out_cleared_by_the_first_register_write() {
        // r16 has PB0 and PB2, for DIR, and r17 is 0, for OUT.
        let run = |writes: [u16; 2]| {
            let image = [ldi(16, ENABLE | PWM), ldi(17, 0)]
                .into_iter()
                .chain(writes)
                .flat_map(u16::to_le_bytes)
                .collect();
            found_lit(Sim::new(image, SRAM.start, Ends::default())?)
        };
        let dir = out(reg::VPORTB_DIR, 16);
        let clear = out(reg::VPORTB_OUT, 17);
        assert_eq!(run([clear, dir]), Ok(()));
        let err = run([dir, clear]).unwrap_err();
        assert!(err.contains("leaves PB2 set"), "{err}");
        assert!(run([ldi(16, 0), ldi(17, 0)]).is_err());
    }

    #[test]
    fn power_down_wants_pb2_low() {
        let mut chip = running();
        chip.write(reg::RTC_PITINTCTRL as u16, RTC_PI).unwrap();
        chip.io[reg::RTC_PITCTRLA] = RTC_PITEN;
        chip.pit_busy = 0;
        wide(&mut chip, reg::TCA0_CMP0BUF, 10).unwrap();
        chip.write(reg::VPORTB_OUT as u16, ENABLE).unwrap();
        chip.write(reg::SLPCTRL_CTRLA as u16, SLEEP_POWER_DOWN | SLPCTRL_SEN)
            .unwrap();
        let err = chip.sleep().unwrap_err();
        assert!(err.contains("PB2 high"), "{err}");

        chip.write(reg::VPORTB_OUT as u16, 0).unwrap();
        wide(&mut chip, reg::TCA0_CMP0BUF, 0).unwrap();
        chip.write(reg::TCA0_CTRLB as u16, TCA_DSBOTTOM).unwrap();
        chip.sleep().unwrap();
    }

    #[test]
    fn the_level_only_changes_where_the_handler_cant_see_half_of_it() {
        let mut chip = running();
        overflow_on(&mut chip);
        for a in [DSM_DUTY, DSM_FRACTION, DSM_SUM] {
            chip.interrupts_on = true;
            let err = chip.write(a as u16, 1).unwrap_err();
            assert!(err.contains("delta-sigma half changed"), "{err}");
            chip.in_handler = true;
            chip.write(a as u16, 1).unwrap();
            chip.in_handler = false;
            chip.interrupts_on = false;
            chip.write(a as u16, 1).unwrap();
        }
        chip.write(reg::TCA0_INTCTRL as u16, 0).unwrap();
        chip.interrupts_on = true;
        chip.write(DSM_DUTY as u16, 2).unwrap();
    }

    #[test]
    fn the_overflow_goes_on_with_its_flag_cleared_first() {
        let stale = |chip: &mut Chip| {
            let err = chip.write(reg::TCA0_INTCTRL as u16, TCA_OVF).unwrap_err();
            assert!(err.contains("still up from before"), "{err}");
        };
        let mut chip = running();
        cycles(&mut chip, 1).unwrap();
        stale(&mut chip);
        overflow_on(&mut chip);
        chip.write(reg::TCA0_INTCTRL as u16, 0).unwrap();
        cycles(&mut chip, 2).unwrap();
        stale(&mut chip);
        chip.write(reg::TCA0_INTFLAGS as u16, TCA_OVF).unwrap();
        cycles(&mut chip, 3).unwrap();
        stale(&mut chip);
        chip.write(reg::TCA0_INTFLAGS as u16, TCA_OVF).unwrap();
        cycles(&mut chip, 1).unwrap();
        assert!(chip.tca.ovf);
        chip.write(reg::TCA0_INTCTRL as u16, TCA_OVF).unwrap();
    }

    #[test]
    fn the_delta_sigma_starts_from_nothing_each_time() {
        let mut chip = running();
        chip.write(DSM_SUM as u16, 5).unwrap();
        let err = chip.write(reg::TCA0_INTCTRL as u16, TCA_OVF).unwrap_err();
        assert!(err.contains("sum at 5"), "{err}");
        chip.write(DSM_SUM as u16, 0).unwrap();
        overflow_on(&mut chip);
    }

    #[test]
    fn idle_with_the_light_on_is_fine_and_wakes_on_the_overflow() {
        let mut chip = running();
        overflow_on(&mut chip);
        chip.tca.ovf = false;
        chip.write(reg::SLPCTRL_CTRLA as u16, SLEEP_IDLE | SLPCTRL_SEN)
            .unwrap();
        chip.sleep().unwrap();
        chip.idle().unwrap();
        assert_eq!(chip.interrupt(), Some(VECTOR_TCA0_OVF));
    }

    #[test]
    fn power_down_stops_tca0_but_not_the_pit() {
        let mut chip = running();
        chip.write(reg::TCA0_CTRLB as u16, TCA_DSBOTTOM).unwrap();
        chip.write(reg::RTC_PITINTCTRL as u16, RTC_PI).unwrap();
        chip.io[reg::RTC_PITCTRLA] = RTC_PITEN;
        chip.pit_busy = 0;
        chip.write(reg::SLPCTRL_CTRLA as u16, SLEEP_POWER_DOWN | SLPCTRL_SEN)
            .unwrap();
        chip.tca.ovf = false;
        chip.sleep().unwrap();
        chip.idle().unwrap();
        assert!(chip.pit_flag);
        assert!(!chip.tca.ovf);
    }

    #[test]
    fn sleeping_with_no_interrupt_enabled_is_stopping_for_good() {
        let mut chip = running();
        chip.write(reg::SLPCTRL_CTRLA as u16, SLEEP_IDLE | SLPCTRL_SEN)
            .unwrap();
        assert_eq!(chip.sleep(), Ok(false));
        assert!(chip.down);

        overflow_on(&mut chip);
        assert_eq!(chip.sleep(), Ok(true));
        assert!(!chip.down);
    }

    fn stop_for_good(first: u16) -> Result<u64, String> {
        let image = [first, 0x9588]
            .into_iter()
            .flat_map(u16::to_le_bytes)
            .collect();
        let mut sim = Sim::new(image, SRAM.start, Ends::default()).unwrap();
        sim.chip.io[reg::SLPCTRL_CTRLA] = SLEEP_IDLE | SLPCTRL_SEN;
        sim.tick(false)
    }

    #[test]
    fn a_pass_that_stops_for_good_fails() {
        let err = stop_for_good(0x94F8).unwrap_err();
        assert!(err.contains("stopped for good"), "{err}");
    }

    #[test]
    fn stopping_for_good_wants_sreg_i_clear_too() {
        let err = stop_for_good(0x9478).unwrap_err();
        assert!(err.contains("with SREG.I still set"), "{err}");
    }

    #[test]
    fn a_stopped_oscillator_stops_the_pit_and_the_watchdog() {
        let mut chip = watched();
        chip.io[reg::RTC_PITCTRLA] = RTC_PITEN;
        chip.osc32k_stopped = true;
        ticks(&mut chip, 100).unwrap();
        assert!(!chip.pit_flag);
        assert_eq!(chip.ticks, 0);

        chip.osc32k_stopped = false;
        ticks(&mut chip, 1).unwrap();
        assert!(chip.pit_flag);
        assert!(ticks(&mut chip, 1).unwrap_err().contains("without wdr"));
    }

    #[test]
    fn a_slow_oscillator_stretches_the_pits_period() {
        let mut chip = Chip::new(Vec::new());
        chip.io[reg::RTC_PITCTRLA] = RTC_PITEN;
        chip.tick_cycles = SLOWEST_TICK_CYCLES;
        ticks(&mut chip, 1).unwrap();
        assert_eq!(chip.ticks, 1);
        chip.advance_to(chip.now + SLOWEST_TICK_CYCLES - 1).unwrap();
        assert_eq!(chip.ticks, 1);
        chip.elapse().unwrap();
        assert_eq!(chip.ticks, 2);
        // 30% slow against 5% fast, in PWM cycles.
        assert_eq!(SLOWEST_TICK_CYCLES.div_ceil(PWM_CYCLE), 460);
    }

    #[test]
    fn tca0_alone_cant_wake_it_from_power_down() {
        let mut chip = running();
        chip.write(reg::TCA0_CTRLB as u16, TCA_DSBOTTOM).unwrap();
        overflow_on(&mut chip);
        chip.pit_busy = 0;
        chip.write(reg::SLPCTRL_CTRLA as u16, SLEEP_POWER_DOWN | SLPCTRL_SEN)
            .unwrap();
        assert!(chip.sleep().unwrap_err().contains("no interrupt"));
    }

    #[test]
    fn a_switch_edge_flags_pa5_when_its_isc_asks() {
        let mut chip = Chip::new(Vec::new());
        chip.write((reg::PORTA_PIN0CTRL + SWITCH_PIN) as u16, ISC_FALLING)
            .unwrap();
        chip.set_switch(true).unwrap();
        assert_eq!(chip.porta_flags, SWITCH);
        chip.write(reg::PORTA_INTFLAGS as u16, SWITCH).unwrap();
        chip.set_switch(false).unwrap();
        assert_eq!(
            chip.porta_flags, 0,
            "a rising edge, which ISC didn't ask for"
        );
    }

    #[test]
    fn pa5_reads_through_its_pull_up_and_a_press_raises_nothing() {
        let mut chip = Chip::new(Vec::new());
        let ctrl = (reg::PORTA_PIN0CTRL + SWITCH_PIN) as u16;
        let read = |chip: &mut Chip| chip.read(reg::VPORTA_IN as u16);
        assert!(read(&mut chip).unwrap_err().contains("battery divider"));
        chip.write(PA1_CTRL as u16, ISC_INPUT_DISABLE).unwrap();
        assert!(read(&mut chip).unwrap_err().contains("pull-up"));
        chip.write(ctrl, PORT_PULLUPEN | ISC_INTDISABLE).unwrap();
        assert_eq!(read(&mut chip), Ok(0xFF));

        chip.set_switch(true).unwrap();
        assert_eq!(read(&mut chip), Ok(!SWITCH));
        assert_eq!(chip.porta_flags, 0, "no sense, so no flag");
        assert_eq!(chip.interrupt(), None);
        chip.write(reg::SLPCTRL_CTRLA as u16, SLEEP_IDLE | SLPCTRL_SEN)
            .unwrap();
        assert_eq!(chip.sleep(), Ok(false), "nothing's enabled to wake it");

        chip.write(ctrl, PORT_PULLUPEN | ISC_INPUT_DISABLE).unwrap();
        assert!(read(&mut chip).unwrap_err().contains("input buffer"));
    }

    #[test]
    fn a_protected_write_wants_the_key() {
        let mut chip = Chip::new(Vec::new());
        let a = reg::CLKCTRL_MCLKCTRLB as u16;
        assert!(chip.write(a, CLOCK_10MHZ).unwrap_err().contains("CCP"));
        chip.write(reg::CPU_CCP as u16, CCP_IOREG).unwrap();
        chip.write(a, CLOCK_10MHZ).unwrap();
        chip.write(reg::CPU_CCP as u16, CCP_IOREG).unwrap();
        assert!(chip.write(a, 0x03).unwrap_err().contains("10 MHz"));
    }

    #[test]
    fn dsm_sum_needs_a_whole_steady_turn_and_compares_with_the_level() {
        let mut chip = running();
        wide(&mut chip, reg::TCA0_CMP0BUF, 10).unwrap();
        chip.write(reg::VPORTB_OUT as u16, ENABLE).unwrap();
        let output = Output {
            enable: true,
            dsm: 10 << DSM_BITS,
        };
        cycles(&mut chip, 3).unwrap();
        assert_eq!(chip.dsm_sum(), None);
        assert_eq!(chip.shows(output), Ok(()), "not settled, so nothing to say");
        cycles(&mut chip, DSM_WINDOW as u64 + 1).unwrap();
        assert_eq!(chip.dsm_sum(), Some(10 << DSM_BITS));
        assert_eq!(chip.shows(output), Ok(()));
        let dimmer = Output {
            dsm: 9 << DSM_BITS,
            ..output
        };
        assert!(chip.shows(dimmer).unwrap_err().contains("add up"));
        assert!(chip.shows(Output::OFF).unwrap_err().contains("PB2"));
    }

    #[test]
    fn a_register_it_doesnt_model_is_an_error() {
        let mut chip = Chip::new(Vec::new());
        assert!(chip.write(0x0A02, 0).unwrap_err().contains("doesn't model"));
        assert!(chip.read(0x0A02).unwrap_err().contains("doesn't model"));
    }

    #[test]
    fn a_direct_compare_write_only_goes_with_pb2_low() {
        let mut chip = running();
        wide(&mut chip, reg::TCA0_CMP0, 0).unwrap();
        wide(&mut chip, reg::TCA0_CMP0BUF, 10).unwrap();
        chip.write(reg::VPORTB_OUT as u16, ENABLE).unwrap();
        let err = wide(&mut chip, reg::TCA0_CMP0, 0).unwrap_err();
        assert!(err.contains("TCA0.CMP0"), "{err}");
    }

    #[test]
    fn with_cmp0en_clear_the_pin_gets_no_duty() {
        let mut chip = running();
        chip.write(reg::TCA0_CTRLB as u16, TCA_DSBOTTOM).unwrap();
        wide(&mut chip, reg::TCA0_CMP0BUF, 10).unwrap();
        chip.write(reg::VPORTB_OUT as u16, ENABLE).unwrap();
        let err = cycles(&mut chip, u64::from(ZERO_DUTY_CYCLES) + 1).unwrap_err();
        assert!(err.contains("duty at 0"), "{err}");
    }

    #[test]
    fn an_aux_pin_is_lit_by_its_pull_up_as_an_input() {
        let mut chip = Chip::new(Vec::new());
        chip.write(reg::VPORTC_DIR as u16, AUX).unwrap();
        chip.write(reg::PORTC_PIN0CTRL as u16 + 3, PORT_PULLUPEN)
            .unwrap();
        assert_eq!(chip.aux(), 0, "an output driven low");
        chip.write(reg::VPORTC_DIR as u16, AUX & !(1 << board::AUX_RED))
            .unwrap();
        assert_eq!(chip.aux(), board::aux_pin(Aux::Red), "red");
        assert_eq!(chip.aux_bright(), 0, "dimly");
    }

    #[test]
    fn an_aux_pin_driven_high_is_lit_brightly() {
        let mut chip = Chip::new(Vec::new());
        chip.write(reg::VPORTC_DIR as u16, AUX).unwrap();
        chip.write(reg::VPORTC_OUT as u16, 1 << board::AUX_RED)
            .unwrap();
        assert_eq!(chip.aux(), board::aux_pin(Aux::Red));
        assert_eq!(chip.aux_bright(), board::aux_bright(Aux::Red));
    }

    fn poke(chip: &mut Chip, a: usize, value: u8) -> Result<(), String> {
        chip.write(a as u16, value)
    }

    fn refused(chip: &mut Chip, a: usize, value: u8) -> String {
        poke(chip, a, value).expect_err("the write went through")
    }

    fn unlocked(chip: &mut Chip, a: usize, value: u8) -> Result<(), String> {
        poke(chip, reg::CPU_CCP, CCP_IOREG)?;
        poke(chip, a, value)
    }

    fn start(chip: &mut Chip) -> Result<(), String> {
        poke(chip, reg::ADC0_COMMAND, ADC_STCONV)
    }

    #[test]
    fn a_conversion_only_starts_set_up_as_the_firmware_sets_it_up() {
        let ctrlc = ADC_SAMPCAP | ADC_REFSEL_INTREF | regs::ADC_PRESC_DIV16;
        for (a, value, why) in [
            (reg::ADC0_CTRLA, 0, "wants the ADC enabled"),
            (
                reg::ADC0_CTRLA,
                ADC_ENABLE | regs::ADC_RESSEL_8BIT,
                "10-bit results",
            ),
            (reg::VREF_CTRLB, 0, "ADC0REFEN clear"),
            (reg::ADC0_CALIB, regs::ADC_DUTYCYC_DUTY50, "25% duty"),
            // 10 MHz / 2 is 5 MHz, and / 256 is 39 kHz.
            (
                reg::ADC0_CTRLC,
                ADC_SAMPCAP | regs::ADC_PRESC_DIV2,
                "at 5000000 Hz",
            ),
            (
                reg::ADC0_CTRLC,
                ADC_SAMPCAP | regs::ADC_PRESC_DIV256,
                "at 39062 Hz",
            ),
            (reg::VREF_CTRLA, regs::VREF_ADC0REFSEL_2V5, "internal 1.1 V"),
            (
                reg::ADC0_CTRLC,
                ctrlc | regs::ADC_REFSEL_VDDREF,
                "internal 1.1 V",
            ),
            (reg::ADC0_CTRLB, 0, "four samples"),
            (reg::ADC0_MUXPOS, regs::ADC_MUXPOS_AIN2, "neither AIN1 nor"),
        ] {
            let mut chip = adc_ready(ADC_MUXPOS_AIN1);
            poke(&mut chip, a, value).unwrap();
            let err = start(&mut chip).unwrap_err();
            assert!(err.contains(why), "{}: {err}", name(a));
        }
    }

    /// Section 30.3.2.6.
    #[test]
    fn the_temperature_sensor_wants_sampcap_and_32_us_of_initdly_and_samplen() {
        for (a, value) in [
            (reg::ADC0_CTRLC, ADC_REFSEL_INTREF | regs::ADC_PRESC_DIV16),
            (reg::ADC0_CTRLD, regs::ADC_INITDLY_DLY16),
            (reg::ADC0_SAMPCTRL, 19),
        ] {
            let mut chip = adc_ready(ADC_MUXPOS_TEMPSENSE);
            poke(&mut chip, a, value).unwrap();
            let err = start(&mut chip).unwrap_err();
            assert!(err.contains("at least 20 clocks"), "{}: {err}", name(a));
            let mut chip = adc_ready(ADC_MUXPOS_AIN1);
            poke(&mut chip, a, value).unwrap();
            start(&mut chip).unwrap();
        }
        let mut chip = adc_ready(ADC_MUXPOS_TEMPSENSE);
        poke(&mut chip, reg::ADC0_SAMPCTRL, 20).unwrap();
        start(&mut chip).unwrap();
    }

    #[test]
    fn a_conversion_doesnt_start_while_one_runs() {
        let mut chip = adc_ready(ADC_MUXPOS_AIN1);
        start(&mut chip).unwrap();
        chip.advance_to(chip.now + CONVERSION_INSTRUCTIONS - 1)
            .unwrap();
        let err = start(&mut chip).unwrap_err();
        assert!(err.contains("still running"), "{err}");
        chip.elapse().unwrap();
        start(&mut chip).unwrap();
    }

    #[test]
    fn the_results_high_byte_needs_its_low_byte_first() {
        let mut chip = adc_ready(ADC_MUXPOS_AIN1);
        start(&mut chip).unwrap();
        chip.advance_to(chip.now + CONVERSION_INSTRUCTIONS).unwrap();
        let err = chip.read(reg::ADC0_RES as u16 + 1).unwrap_err();
        assert!(err.contains("without its low byte"), "{err}");
        chip.read(reg::ADC0_RES as u16).unwrap();
        chip.read(reg::ADC0_RES as u16 + 1).unwrap();
        assert!(chip.read(reg::ADC0_RES as u16 + 1).is_err());
    }

    #[test]
    fn only_the_whole_flash_priority_scan_is_modelled() {
        let mut chip = Chip::new(Vec::new());
        let err = refused(
            &mut chip,
            reg::CRCSCAN_CTRLA,
            CRCSCAN_ENABLE | regs::CRCSCAN_NMIEN,
        );
        assert!(err.contains("only models ENABLE"), "{err}");
        poke(&mut chip, reg::CRCSCAN_CTRLB, regs::CRCSCAN_SRC_BOOT).unwrap();
        let err = refused(&mut chip, reg::CRCSCAN_CTRLA, CRCSCAN_ENABLE);
        assert!(err.contains("whole-flash"), "{err}");
    }

    #[test]
    fn a_chip_left_on_the_wrong_clock_or_pwm_is_refused() {
        let mut chip = Chip::new(Vec::new());
        let err = chip.clocked_as_modelled().unwrap_err();
        assert!(err.contains("never set the clock"), "{err}");
        unlocked(&mut chip, reg::CLKCTRL_MCLKCTRLB, CLOCK_10MHZ).unwrap();
        let err = chip.clocked_as_modelled().unwrap_err();
        assert!(err.contains("never started TCA0"), "{err}");
        poke(&mut chip, reg::TCA0_CTRLA, TCA_RUNNING).unwrap();
        let err = chip.clocked_as_modelled().unwrap_err();
        assert!(err.contains("TCA0.PER is 65535"), "{err}");

        let mut chip = running();
        unlocked(&mut chip, reg::CLKCTRL_MCLKCTRLB, CLOCK_10MHZ).unwrap();
        chip.clocked_as_modelled().unwrap();
    }

    #[test]
    fn the_key_lasts_four_instructions_and_one_write() {
        let a = reg::CLKCTRL_MCLKCTRLB;
        let mut chip = Chip::new(Vec::new());
        poke(&mut chip, reg::CPU_CCP, CCP_IOREG).unwrap();
        assert!(chip.holds_interrupts());
        chip.advance_to(chip.now + 4).unwrap();
        assert!(chip.holds_interrupts());
        poke(&mut chip, a, CLOCK_10MHZ).unwrap();
        assert!(!chip.holds_interrupts(), "the write ends the hold");
        let err = refused(&mut chip, a, CLOCK_10MHZ);
        assert!(err.contains("without the CCP key"), "spent: {err}");

        poke(&mut chip, reg::CPU_CCP, CCP_IOREG).unwrap();
        chip.advance_to(chip.now + 5).unwrap();
        assert!(!chip.holds_interrupts());
        let err = refused(&mut chip, a, CLOCK_10MHZ);
        assert!(err.contains("MCLKCTRLB without the CCP key"), "{err}");

        let err = refused(&mut chip, reg::WDT_CTRLA, 0x33);
        assert!(err.contains("WDT.CTRLA without the CCP key"), "{err}");
    }

    #[test]
    fn only_the_ioreg_key_goes_in_ccp() {
        let mut chip = Chip::new(Vec::new());
        let err = refused(&mut chip, reg::CPU_CCP, regs::CPU_CCP_SPM);
        assert!(err.contains("isn't the IOREG key"), "{err}");
    }

    #[test]
    fn power_down_wants_no_duty_waiting_no_conversion_running_and_the_pit_synced() {
        let mut chip = running();
        chip.pit_busy = 0;
        let err = chip.ready_for_power_down().unwrap_err();
        assert!(err.contains("CMP0EN set"), "{err}");
        chip.write(reg::TCA0_CTRLB as u16, TCA_DSBOTTOM).unwrap();
        chip.ready_for_power_down().unwrap();
        wide(&mut chip, reg::TCA0_CMP0BUF, 10).unwrap();
        let err = chip.ready_for_power_down().unwrap_err();
        assert!(err.contains("CMP0BUF at 10"), "{err}");
        cycles(&mut chip, 1).unwrap();
        wide(&mut chip, reg::TCA0_CMP0BUF, 0).unwrap();
        let err = chip.ready_for_power_down().unwrap_err();
        assert!(err.contains("CMP0 at 10"), "{err}");
        wide(&mut chip, reg::TCA0_CMP0, 0).unwrap();
        chip.ready_for_power_down().unwrap();

        let mut chip = adc_ready(ADC_MUXPOS_AIN1);
        chip.pit_busy = 0;
        start(&mut chip).unwrap();
        let err = chip.ready_for_power_down().unwrap_err();
        assert!(err.contains("mid-conversion"), "{err}");

        let chip = Chip::new(Vec::new());
        let err = chip.ready_for_power_down().unwrap_err();
        assert!(err.contains("CTRLBUSY"), "{err}");
    }

    #[test]
    fn pa5_isnt_inverted_and_only_pb2_is_driven_by_hand() {
        let mut chip = Chip::new(Vec::new());
        let pa5 = reg::PORTA_PIN0CTRL + SWITCH_PIN;
        let err = refused(&mut chip, pa5, PORT_PULLUPEN | PORT_INVEN);
        assert!(err.contains("INVEN on PA5"), "{err}");
        let err = refused(&mut chip, reg::VPORTB_OUT, ENABLE | PWM);
        assert!(err.contains("only PB2 is driven by hand"), "{err}");
    }

    #[test]
    fn a_switch_sense_the_model_doesnt_have_is_refused() {
        let mut chip = Chip::new(Vec::new());
        // ISC only goes up to LEVEL, and 6 and 7 are reserved.
        poke(&mut chip, reg::PORTA_PIN0CTRL + SWITCH_PIN, 0x06).unwrap();
        let err = chip.set_switch(true).unwrap_err();
        assert!(err.contains("PA5's ISC is 0x6"), "{err}");
    }

    #[test]
    fn per_is_pwm_top_and_only_written_directly_with_the_timer_stopped() {
        for low in [reg::TCA0_PER, reg::TCA0_PERBUF] {
            let mut chip = Chip::new(Vec::new());
            let err = wide(&mut chip, low, PWM_TOP - 1).unwrap_err();
            assert!(err.contains("assume 255"), "{}: {err}", name(low));
        }
        let mut chip = Chip::new(Vec::new());
        wide(&mut chip, reg::TCA0_PER, PWM_TOP).unwrap();
        assert_eq!(chip.tca.per, PWM_TOP);
        let mut chip = running();
        let err = wide(&mut chip, reg::TCA0_PER, PWM_TOP).unwrap_err();
        assert!(err.contains("use PERBUF"), "{err}");
    }

    #[test]
    fn one_wide_register_at_a_time_goes_through_temp() {
        let mut chip = running();
        poke(&mut chip, reg::TCA0_CMP0BUF, 10).unwrap();
        let err = refused(&mut chip, reg::TCA0_PERBUF, 0xFF);
        assert!(
            err.contains("TCA0.CMP0BUF's is still waiting for its high byte"),
            "{err}"
        );
        let err = refused(&mut chip, reg::TCA0_PERBUF + 1, 0);
        assert!(err.contains("TCA0.PERBUF without its low byte"), "{err}");
    }

    #[test]
    fn tca0_only_runs_undivided_and_dual_slope_at_bottom() {
        let mut chip = Chip::new(Vec::new());
        for value in [0, regs::TCA_CLKSEL_DIV2 | regs::TCA_ENABLE] {
            let err = refused(&mut chip, reg::TCA0_CTRLA, value);
            assert!(err.contains("runs undivided"), "{err}");
        }
        for value in [
            regs::TCA_WGMODE_SINGLESLOPE | TCA_CMP0EN,
            TCA_DSBOTTOM | regs::TCA_CMP2EN,
        ] {
            let err = refused(&mut chip, reg::TCA0_CTRLB, value);
            assert!(err.contains("dual-slope at BOTTOM on WO0"), "{err}");
        }
    }

    #[test]
    fn the_pits_setup_waits_for_ctrlbusy() {
        let mut chip = Chip::new(Vec::new());
        let err = refused(&mut chip, reg::RTC_PITCTRLA, RTC_PITEN);
        assert!(err.contains("CTRLBUSY is set"), "{err}");
        while chip.read(reg::RTC_PITSTATUS as u16).unwrap() != 0 {}
        poke(&mut chip, reg::RTC_PITCTRLA, RTC_PITEN).unwrap();
        assert!(poke(&mut chip, reg::RTC_PITCTRLA, RTC_PITEN).is_err());
    }

    #[test]
    fn the_watchdogs_setup_waits_for_syncbusy_and_takes_no_reserved_value() {
        let mut chip = Chip::new(Vec::new());
        for value in [WDT_PERIOD_MAX + 1, (WDT_PERIOD_MAX + 1) << 4 | 0x03] {
            let err = unlocked(&mut chip, reg::WDT_CTRLA, value).unwrap_err();
            assert!(err.contains("reserved"), "{err}");
        }
        unlocked(&mut chip, reg::WDT_CTRLA, 0x33).unwrap();
        let err = unlocked(&mut chip, reg::WDT_CTRLA, 0x33).unwrap_err();
        assert!(err.contains("SYNCBUSY is set"), "{err}");
    }

    #[test]
    fn wdt_status_is_never_written() {
        let mut chip = Chip::new(Vec::new());
        let err = refused(&mut chip, reg::WDT_STATUS, WDT_LOCK);
        assert!(err.contains("sets WDT.STATUS.LOCK"), "{err}");
        let err = refused(&mut chip, reg::WDT_STATUS, 0);
        assert!(err.contains("only writable bit is LOCK"), "{err}");
    }

    #[test]
    fn the_watchdogs_periods_left_stop_one_short_of_the_bite() {
        let mut chip = Chip::new(Vec::new());
        assert_eq!(chip.wdt_periods_left(), DOWN_PERIODS, "off");
        // 64 clocks, four ticks, the fourth being the bite.
        unlocked(&mut chip, reg::WDT_CTRLA, regs::WDT_PERIOD_64CLK).unwrap();
        assert_eq!(chip.wdt_periods_left(), 3);
        ticks(&mut chip, 3).unwrap();
        assert_eq!(chip.wdt_periods_left(), 0);
        assert!(ticks(&mut chip, 1).is_err());
    }

    #[test]
    fn sleep_wants_sen_and_a_mode_the_model_has() {
        let mut chip = Chip::new(Vec::new());
        let err = chip.sleep().unwrap_err();
        assert!(err.contains("SEN clear"), "{err}");
        let standby = regs::SLPCTRL_SMODE_STDBY | SLPCTRL_SEN;
        poke(&mut chip, reg::SLPCTRL_CTRLA, standby).unwrap();
        let err = chip.sleep().unwrap_err();
        assert!(err.contains("only models idle and power-down"), "{err}");
    }

    const IN_ORDER: Boot = Boot {
        flags_cleared: Some(1),
        flash_checked: Some(2),
        fuses_read: Some(3),
        set_up: Some(4),
        pit_on: Some(5),
        watchdog_on: Some(6),
    };

    #[test]
    fn start_up_has_to_do_every_step() {
        IN_ORDER.in_order().unwrap();
        let missing = |boot: Boot| boot.in_order().unwrap_err();
        for (boot, what) in [
            (
                Boot {
                    flags_cleared: None,
                    ..IN_ORDER
                },
                "never cleared RSTCTRL.RSTFR",
            ),
            (
                Boot {
                    flash_checked: None,
                    ..IN_ORDER
                },
                "never started a CRC scan",
            ),
            (
                Boot {
                    fuses_read: None,
                    ..IN_ORDER
                },
                "never read the fuses",
            ),
            (
                Boot {
                    set_up: None,
                    ..IN_ORDER
                },
                "never set anything up",
            ),
            (
                Boot {
                    pit_on: None,
                    ..IN_ORDER
                },
                "never turned the PIT on",
            ),
            (
                Boot {
                    watchdog_on: None,
                    ..IN_ORDER
                },
                "never turned the watchdog on",
            ),
        ] {
            assert_eq!(missing(boot), what);
        }
    }

    #[test]
    fn start_up_has_to_be_in_order() {
        for (boot, why) in [
            (
                Boot {
                    flags_cleared: Some(7),
                    ..IN_ORDER
                },
                "before clearing the reset flags",
            ),
            (
                Boot {
                    flash_checked: Some(5),
                    ..IN_ORDER
                },
                "before the CRC scan",
            ),
            (
                Boot {
                    fuses_read: Some(5),
                    ..IN_ORDER
                },
                "before reading the fuses",
            ),
            (
                Boot {
                    pit_on: Some(7),
                    ..IN_ORDER
                },
                "before the PIT",
            ),
        ] {
            let err = boot.in_order().unwrap_err();
            assert!(err.contains(why), "{err}");
        }
    }

    #[test]
    fn the_chip_notes_each_step_of_start_up_as_it_happens() {
        let mut chip = Chip::new(Vec::new());
        let step = |chip: &mut Chip| {
            chip.elapse().unwrap();
            chip.now
        };
        poke(&mut chip, reg::VPORTB_OUT, 0).unwrap();
        poke(&mut chip, reg::VPORTB_DIR, ENABLE | PWM).unwrap();
        let flags = step(&mut chip);
        poke(&mut chip, reg::RSTCTRL_RSTFR, RSTCTRL_PORF).unwrap();
        let flash = step(&mut chip);
        poke(&mut chip, reg::CRCSCAN_CTRLB, CRCSCAN_FLASH_PRIORITY).unwrap();
        poke(&mut chip, reg::CRCSCAN_CTRLA, CRCSCAN_ENABLE).unwrap();
        let fuses = step(&mut chip);
        chip.read(regs::addr::FUSE_OSCCFG as u16).unwrap();
        assert_eq!(chip.boot.set_up, None);
        let set_up = step(&mut chip);
        poke(&mut chip, reg::CPU_CCP, CCP_IOREG).unwrap();
        poke(&mut chip, reg::RTC_PITCTRLA + 1, 0).unwrap_err();
        chip.pit_busy = 0;
        let pit = step(&mut chip);
        poke(&mut chip, reg::RTC_PITCTRLA, RTC_PITEN).unwrap();
        let watchdog = step(&mut chip);
        unlocked(&mut chip, reg::WDT_CTRLA, 0x33).unwrap();

        let boot = &chip.boot;
        assert_eq!(boot.flags_cleared, Some(flags));
        assert_eq!(boot.flash_checked, Some(flash));
        assert_eq!(boot.fuses_read, Some(fuses));
        assert_eq!(boot.set_up, Some(set_up));
        assert_eq!(boot.pit_on, Some(pit));
        assert_eq!(boot.watchdog_on, Some(watchdog));
        boot.in_order().unwrap();
    }

    fn statics() -> Layout {
        Layout {
            data: SRAM.start..SRAM.start + 3,
            bss: SRAM.start + 3..SRAM.start + 5,
            load: MAPPED,
        }
    }

    #[test]
    fn startup_has_to_copy_data_and_zero_bss() {
        let layout = statics();
        let mut chip = Chip::new(vec![1, 2, 3]);
        let err = check_startup(&chip, &layout).unwrap_err();
        assert!(err.contains(".data in SRAM doesn't match"), "{err}");
        for (at, value) in (layout.data.start..).zip([1, 2, 3]) {
            chip.write(at, value).unwrap();
        }
        chip.write(layout.bss.start, 0).unwrap();
        let err = check_startup(&chip, &layout).unwrap_err();
        assert_eq!(err, ".bss isn't zeroed: 0x3804 holds 0xa5");
        chip.write(layout.bss.start + 1, 0).unwrap();
        check_startup(&chip, &layout).unwrap();

        let past = Layout {
            load: MAPPED + 1,
            ..statics()
        };
        let err = check_startup(&chip, &past).unwrap_err();
        assert!(err.contains("outside flash"), "{err}");
    }

    /// `call`, to word 0.
    const CALL: [u16; 2] = [0x940E, 0];

    fn loads(layout: &Layout) -> Vec<u16> {
        [
            (26, layout.data.start),
            (30, layout.load),
            (24, layout.data.end),
            (26, layout.bss.start),
            (24, layout.bss.end),
        ]
        .into_iter()
        .flat_map(|(r, value)| {
            let [lo, hi] = value.to_le_bytes();
            [ldi(r, lo), ldi(r + 1, hi)]
        })
        .collect()
    }

    fn image(words: impl IntoIterator<Item = u16>) -> Vec<u8> {
        words.into_iter().flat_map(u16::to_le_bytes).collect()
    }

    #[test]
    fn patching_startup_wants_the_loads_it_knows_then_the_call() {
        let linked = statics();
        let moved = Layout {
            data: 0x3911..0x3916,
            bss: 0x3916..0x391D,
            load: MAPPED + 0x123,
        };
        let mut patched = image(loads(&linked).into_iter().chain(CALL));
        patch_startup(&mut patched, 0, &linked, &moved).unwrap();
        assert_eq!(patched, image(loads(&moved).into_iter().chain(CALL)));

        let mut swapped = loads(&linked);
        swapped.swap(0, 2);
        let mut swapped = image(swapped.into_iter().chain(CALL));
        let before = swapped.clone();
        let err = patch_startup(&mut swapped, 0, &linked, &moved).unwrap_err();
        assert!(err.contains("update `patch_startup`"), "{err}");
        assert_eq!(swapped, before, "nothing's patched");

        let mut no_call = image(loads(&linked));
        let err = patch_startup(&mut no_call, 0, &linked, &moved).unwrap_err();
        assert!(err.contains("no call to main"), "{err}");
    }

    const NOP: u16 = 0x0000;
    const SEI: u16 = 0x9478;
    const SEC: u16 = 0x9408;
    const SLEEP: u16 = 0x9588;
    const RETI: u16 = 0x9518;
    /// `push r0`.
    const PUSH_R0: u16 = 0x920F;
    /// `rjmp` to itself.
    const SPIN: u16 = 0xCFFF;

    /// `jmp`, to a byte address in the first 128 KB.
    fn jmp(to: u32) -> [u16; 2] {
        [0x940C, (to / 2) as u16]
    }

    /// `sts a, r16`.
    fn sts_r16(a: usize) -> [u16; 2] {
        [0x9300, a as u16]
    }

    fn sim(words: impl IntoIterator<Item = u16>) -> Sim {
        let ends = Ends {
            unhandled: 0x1000,
            halt: 0x2000,
        };
        Sim::new(image(words), SRAM.start, ends).unwrap()
    }

    #[test]
    fn an_image_bigger_than_flash_is_refused() {
        let ends = Ends::default();
        Sim::new(vec![0; FLASH_SIZE], SRAM.start, ends)
            .map(drop)
            .unwrap();
        let err = Sim::new(vec![0; FLASH_SIZE + 1], SRAM.start, ends)
            .err()
            .unwrap();
        assert_eq!(err, "16385 bytes won't fit in flash");
    }

    #[test]
    fn a_push_into_bss_is_refused() {
        let run = |bss_end: u16| {
            let mut sim = Sim::new(image([PUSH_R0, PUSH_R0]), bss_end, Ends::default())?;
            sim.step()?;
            assert_eq!(sim.stack(), 1);
            sim.step()
        };
        assert_eq!(run(SRAM.end - 2), Ok(()));
        let err = run(SRAM.end - 1).unwrap_err();
        assert_eq!(err, "0x0002: the stack ran into .bss");
    }

    #[test]
    fn enabling_an_interrupt_with_no_handler_is_refused_once_sreg_i_is_set() {
        let mut sim = sim([NOP, SEI, NOP]);
        poke(&mut sim.chip, reg::RTC_PITINTCTRL, RTC_PI).unwrap();
        sim.step().unwrap();
        let err = sim.step().unwrap_err();
        assert!(
            err.contains("enables RTC_PIT (vector 7) with SREG.I set"),
            "{err}"
        );
    }

    fn handled_by(vector: u8, handler: &[u16]) -> Vec<u16> {
        let slot = usize::from(vector) * VECTOR_WORDS as usize;
        let after = VECTORS * VECTOR_WORDS as usize;
        let mut words = vec![NOP; after];
        words[0] = SEI;
        words[slot..slot + 2].copy_from_slice(&jmp(after as u32 * 2));
        words.extend_from_slice(handler);
        words
    }

    fn handle(body: &[u16]) -> Result<Sim, String> {
        let handler: Vec<u16> = body.iter().copied().chain([RETI]).collect();
        let mut sim = sim(handled_by(VECTOR_TCA0_OVF, &handler));
        poke(&mut sim.chip, reg::TCA0_INTCTRL, TCA_OVF)?;
        sim.chip.tca.ovf = true;
        while sim.cpu.handling.is_none() {
            sim.step()?;
        }
        while sim.cpu.handling.is_some() {
            sim.step()?;
        }
        Ok(sim)
    }

    #[test]
    fn a_handler_gives_back_every_register_sreg_and_sp() {
        let sim = handle(&[NOP, NOP]).map_err(drop).unwrap();
        assert_eq!(sim.longest_handler, 3, "the jmp and the two nops");
        for (body, why) in [
            (ldi(16, 1), "r16 at 0x01, not the 0x00 it found"),
            (SEC, "SREG at 0x81, not the 0x80 it found"),
            (PUSH_R0, "SP at 0x3ffe, having found it at 0x3fff"),
        ] {
            let why = format!("returns with {why}");
            let err = handle(&[body]).err().unwrap();
            assert_eq!(err, format!("the handler for TCA0_OVF (vector 8): {why}"));
        }
    }

    #[test]
    fn a_handler_has_to_fit_in_a_pwm_cycle() {
        let err = handle(&[SPIN]).err().unwrap();
        assert!(err.contains("ran past 125 instructions"), "{err}");
    }

    #[test]
    fn taking_an_interrupt_into_unhandled_interrupt_is_refused() {
        let words = handled_by(VECTOR_RTC_PIT, &[RETI]);
        let ends = Ends {
            unhandled: u32::from(VECTOR_RTC_PIT) * VECTOR_WORDS * 2,
            halt: 0x2000,
        };
        let mut sim = Sim::new(image(words), SRAM.start, ends).unwrap();
        poke(&mut sim.chip, reg::RTC_PITINTCTRL, RTC_PI).unwrap();
        sim.chip.pit_flag = true;
        sim.step().unwrap();
        sim.step().unwrap();
        let err = sim.step().unwrap_err();
        assert_eq!(
            err,
            "takes RTC_PIT (vector 7), whose vector goes to __unhandled_interrupt"
        );
    }

    #[test]
    fn code_that_never_gets_there_runs_out_of_budget() {
        let err = sim([SPIN]).run_to(5, 10).unwrap_err();
        assert_eq!(err, "didn't reach 0x000a within 10 instructions");
        let err = sim([SPIN]).tick(false).unwrap_err();
        assert!(err.contains("ran past 39062 instructions"), "{err}");
        let err = found_lit(sim([SPIN])).unwrap_err();
        assert!(err.contains("wrote no register"), "{err}");
        let err = boot_order(sim([SPIN])).unwrap_err();
        assert!(err.contains("didn't wait for a tick"), "{err}");
    }

    #[test]
    fn start_up_is_judged_at_the_first_wait_for_a_tick() {
        let mut sim = sim([SLEEP]);
        sim.chip.io[reg::SLPCTRL_CTRLA] = SLEEP_IDLE | SLPCTRL_SEN;
        let err = boot_order(sim).unwrap_err();
        assert_eq!(err, "never cleared RSTCTRL.RSTFR");
    }

    /// r16 is set up beforehand so the PIT handler has nothing to give back.
    fn ticking() -> Sim {
        let handler = sts_r16(reg::RTC_PITINTFLAGS).into_iter().chain([RETI]);
        let mut words = handled_by(VECTOR_RTC_PIT, &handler.collect::<Vec<_>>());
        words[1] = SLEEP;
        words[2] = SPIN - 1;
        let mut sim = sim(words);
        sim.cpu.r[16] = RTC_PI;
        let chip = &mut sim.chip;
        unlocked(chip, reg::CLKCTRL_MCLKCTRLB, CLOCK_10MHZ).unwrap();
        wide(chip, reg::TCA0_PER, PWM_TOP).unwrap();
        poke(chip, reg::TCA0_CTRLB, TCA_DSBOTTOM).unwrap();
        poke(chip, reg::TCA0_CTRLA, TCA_RUNNING).unwrap();
        poke(chip, reg::RTC_PITINTCTRL, RTC_PI).unwrap();
        chip.pit_busy = 0;
        poke(chip, reg::RTC_PITCTRLA, RTC_PITEN).unwrap();
        poke(chip, reg::SLPCTRL_CTRLA, SLEEP_IDLE | SLPCTRL_SEN).unwrap();
        unlocked(chip, reg::WDT_CTRLA, WDT_PERIOD_MAX).unwrap();
        poke(chip, reg::VPORTC_DIR, AUX & !board::aux_pin(Aux::Blue)).unwrap();
        poke(chip, BLUE_CTRL, PORT_PULLUPEN).unwrap();
        sim
    }

    const BLUE_CTRL: usize = reg::PORTC_PIN0CTRL + board::AUX_BLUE as usize;

    fn quiet(ticks: usize) -> impl Iterator<Item = Sample> {
        iter::repeat_n(
            Sample {
                pressed: false,
                scene: Scene::NORMAL,
            },
            ticks,
        )
    }

    fn in_step_fails(sim: &mut Sim, ticks: usize) -> String {
        in_step(sim, quiet(ticks), &mut Report::default()).unwrap_err()
    }

    #[test]
    fn in_step_wants_the_clock_the_pwm_and_the_watchdog_set_up_by_the_first_tick() {
        let err = in_step_fails(&mut sim([SPIN]), 1);
        assert!(err.starts_with("hw::init: a pass of the main"), "{err}");

        let mut sim = ticking();
        sim.chip.io[reg::CLKCTRL_MCLKCTRLB] = 0;
        let err = in_step_fails(&mut sim, 1);
        assert_eq!(err, "hw::init: never set the clock to 10 MHz");

        let mut sim = ticking();
        sim.chip.io[reg::WDT_CTRLA] = 0;
        let err = in_step_fails(&mut sim, 1);
        assert_eq!(err, "hw::start_watchdog never turned the watchdog on");
    }

    #[test]
    fn in_step_holds_the_sleep_to_the_librarys() {
        let err = in_step_fails(&mut ticking(), 8);
        assert!(
            err.contains("the library says power down, but the firmware idled"),
            "{err}"
        );
    }

    #[test]
    fn in_step_holds_the_aux_pins_to_the_librarys() {
        let mut sim = ticking();
        poke(&mut sim.chip, BLUE_CTRL, 0).unwrap();
        let err = in_step_fails(&mut sim, 8);
        assert!(
            err.contains("the aux pins are 0b0000, but the library says Blue"),
            "{err}"
        );
    }

    #[test]
    fn in_step_holds_the_aux_brightness_to_the_librarys() {
        let mut sim = ticking();
        let blue = board::aux_pin(Aux::Blue);
        poke(&mut sim.chip, reg::VPORTC_DIR, AUX).unwrap();
        poke(&mut sim.chip, reg::VPORTC_OUT, blue).unwrap();
        let err = in_step_fails(&mut sim, 8);
        assert!(
            err.contains("the bright aux pins are 0b0010, but the library says Blue"),
            "{err}"
        );
    }

    #[test]
    fn in_step_holds_pb2_to_the_librarys() {
        let mut sim = ticking();
        wide(&mut sim.chip, reg::TCA0_CMP0BUF, 10).unwrap();
        poke(&mut sim.chip, reg::TCA0_CTRLB, TCA_DSBOTTOM | TCA_CMP0EN).unwrap();
        poke(&mut sim.chip, reg::VPORTB_DIR, ENABLE | PWM).unwrap();
        poke(&mut sim.chip, reg::VPORTB_OUT, ENABLE).unwrap();
        let err = in_step_fails(&mut sim, 8);
        assert!(err.contains("PB2 is true, but the library says"), "{err}");
    }

    #[test]
    fn a_firmware_that_never_feeds_the_watchdog_is_bitten() {
        let mut sim = ticking();
        // 64 clocks, four ticks, the fourth being the bite.
        sim.chip.io[reg::WDT_CTRLA] = regs::WDT_PERIOD_64CLK;
        for _ in 0..3 {
            sim.tick(false).unwrap();
        }
        let err = sim.tick(false).unwrap_err();
        assert!(err.starts_with("asleep: the watchdog went 4"), "{err}");
    }

    #[test]
    fn a_failed_run_says_which_it_was_and_whether_overflows_were_landing() {
        let power_on = || Ok(ticking());
        let failed = "the library says power down, but the firmware idled";
        // Ticks 0 and 1 read the cell, so tick 2's the first it rests.
        let err = run(&power_on, None, Landing::Watch).err().unwrap();
        assert_eq!(err, format!("random switching: tick 2: {failed}"));
        let err = run(&power_on, None, Landing::Until(5)).err().unwrap();
        assert_eq!(
            err,
            format!("with overflows mid-pass: random switching: tick 2: {failed}")
        );

        let example = &EXAMPLES[0];
        let err = run(&power_on, Some(example), Landing::Watch).err().unwrap();
        assert!(
            err.starts_with(&format!("{}: tick ", example.name)),
            "{err}"
        );
    }

    #[test]
    fn the_forced_interrupt_has_to_be_one_with_no_handler() {
        let err = unhandled_interrupt(sim(handled_by(VECTOR_UNHANDLED, &[RETI]))).unwrap_err();
        assert_eq!(
            err,
            "vector 13 has a handler now, so pick another vector to force"
        );
    }
}
