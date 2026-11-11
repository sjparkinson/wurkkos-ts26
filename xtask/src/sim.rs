//! A fake ATtiny1616 around `avr.rs`'s CPU, wired up like the TS26's driver,
//! and the checks that run the real firmware on it. `build()` in main.rs runs
//! them after `layout.rs`'s, so `hex` and `flash` refuse firmware that fails.
//!
//! This finds mistakes, it doesn't prove anything. Everything here is my
//! reading of the datasheet, run against code written from that same reading,
//! and nothing has been compared with a real chip. The TS21 had a far bigger
//! simulator than this, and it didn't save that torch. The likely cause is
//! the 12 V programmer or a slipped probe, not proven, and no model of the
//! chip can see either. A pass means the firmware does what this file thinks
//! it should.
//!
//! It models only the registers the firmware touches, and turns the rules the
//! real chip enforces quietly into errors. Touching a register it doesn't
//! model is an error too, so new peripheral code fails here until the model
//! learns about it.
//!
//! - A protected register only takes a write within four instructions of the
//!   CCP key. Interrupts wait out those four instructions, or until the first
//!   store or `sleep` (§8.5.7).
//! - TCA0 runs dual-slope at BOTTOM, undivided, with PER at `PWM_TOP`: a cycle
//!   is 2 × PER clocks, so 510 clocks, 19.6 kHz at 10 MHz (§20.3.3.4.4 gives
//!   the frequency; the clock count is worked back from it). Each BOTTOM sets
//!   INTFLAGS.OVF and copies PERBUF and CMP0BUF to the live registers if
//!   they've been written (table 20-6). The datasheet doesn't say whether those
//!   land on the same clock, nor what a BUF write on that very clock does, so
//!   neither's modelled. Anything else in CTRLA or CTRLB but CMP0EN is refused,
//!   and so is a compare over PER, whose dual-slope behaviour it doesn't give.
//!   CMP0 can be written directly with PB2 low. With CMP0EN clear PB0 is
//!   PORT's, so it gets no duty. Whether a timer that's just been enabled
//!   counts as being at BOTTOM isn't in the datasheet either, so this guesses
//!   it doesn't and runs the whole first cycle at whatever PER is.
//! - TCA0's 16-bit registers go low byte then high byte through one shared
//!   TEMP byte. A handler that lands with the main loop halfway through one
//!   can't touch any of them, since it'd overwrite that byte (§8.5.6, §20.5.13).
//!   Only a handler may write CMP0BUF with the boost converter on.
//! - A handler gives back every register, SREG and SP as it found them, and
//!   has to fit in a PWM cycle even if every instruction took four clocks.
//! - PB2 is only ever an output with PORTB's other bits clear. It may not go
//!   high with no duty and nothing to supply one, nor stay high with the
//!   duty at 0 for longer than the delta-sigma's longest gap, and it has to be
//!   low in power-down. PB2 and PA1 never get a pull-up, and PA0's settings
//!   are never written at all.
//! - PA1 is an ADC input, so its digital input buffer has to be off, and
//!   nothing else set, before AIN1 is converted (§30.3.1.1). The buffer has
//!   to be off before VPORTA.IN is read too, since a read of the port with it
//!   on is a digital read of the battery divider.
//! - RTC.PITCTRLA isn't written while PITSTATUS.CTRLBUSY is set.
//! - The PIT's flag goes up once a period of simulated time, which counts an
//!   instruction at a time while the CPU's awake and skips to the next event
//!   while it sleeps. A run can stretch the period to the slowest the two
//!   oscillators' limits allow (tables 36-12 and 36-13), or stop the 32 kHz
//!   oscillator, which stops the PIT and the watchdog together.
//! - Interrupts come from TCA0's OVF and the PIT. The model has PORTA's
//!   pin-change flags too, which a switch edge sets when PA5's ISC asks for
//!   that edge, but the firmware leaves ISC at INTDISABLE, so none goes up and
//!   the switch never wakes it. Enabling an interrupt with SREG.I set and its
//!   vector going to `__unhandled_interrupt` is an error: the real chip would
//!   return with the flag still up, forever. PORTA's vector is one of those.
//! - The switch is read through VPORTA.IN, which wants PA5's input buffer on
//!   and its pull-up, since the switch only pulls the pin low. IN follows the
//!   pin at once here. The real one stops following it in power-down, and the
//!   datasheet doesn't say how soon after a wake it's caught up (§16.3.5).
//! - `sleep` needs SLPCTRL.SEN, idle or power-down, and an enabled interrupt
//!   that could wake it, which in power-down leaves out TCA0 (it stops) and
//!   RISING and FALLING on all but pins 2 and 6 (table 16-3). Power-down also
//!   wants no conversion running, the ADC off and its reference not forced on.
//!   With no interrupt enabled at all, `sleep` is the chip stopping for good,
//!   since only a reset wakes it (§11.3.1). That's `hw::halt`, and it fails
//!   every run but the one that stops the oscillator. `sleep` with SREG.I
//!   clear and an interrupt still enabled is an error: only a block diagram
//!   says that interrupt couldn't wake it (figure 13-1, and `avr.rs`).
//! - The watchdog: WDT.CTRLA takes the CCP key and isn't written while
//!   SYNCBUSY is set, and nothing may change it once it's on. A `wdr` in the
//!   closed window, or on its edge, fails, and so does running out the open
//!   window, as the real chip would reset either way. Nothing here resets the
//!   chip, so a bite is the firmware failing.
//! - ADC0 only converts as one of the two measurements the firmware makes,
//!   AIN1 or the temperature sensor against 1.1 V, four accumulated, then reads
//!   the scripted world (`world.rs`). Reading RES early, or its high byte
//!   first, starting mid-conversion or changing its setup mid-conversion are
//!   errors, and the first conversion after a change of input reads 0.
//! - CRCSCAN runs CRC-16-CCITT over all of flash and sets STATUS.OK if it comes
//!   to 0, which is what a correct checksum in the last two bytes does
//!   (`crc.rs`). The firmware runs on the image `cargo xtask hex` writes.
//! - The fuses read as the world's, and writing any of them is an error.
//!   SIGROW.TEMPSENSE0 and 1 hold `world::CALIBRATION`.
//!
//! Time counts instructions, not cycles, so a real chip gets fewer of them
//! into a tick or a PWM cycle than this does. The budgets allow for that by
//! taking every instruction as the slowest, four clocks.
//!
//! With that in place it runs:
//!
//! 1. Reset to `main` with SRAM full of junk, checking .data and .bss come out
//!    right, then the same startup code with them moved and given contents.
//! 2. The reset vector again with PB2 already an output and high, as far as
//!    the first register write, which has to clear it (`found_lit`). Then
//!    again as far as the first wait for a tick, checking `main` starts up in
//!    the order it has to (`boot_order`).
//! 3. Two seconds lit with every tick at its slowest, which mustn't trip
//!    `hw::wait_for_tick`'s deadline (`slow_ticks`).
//! 4. The 32 kHz oscillator stopping with the light on. PB2 has to drop within
//!    the deadline, and the chip then stop for good in power-down
//!    (`oscillator_stops`). A tick lost with the oscillator still running ends
//!    in a watchdog reset instead, which can't run here.
//! 5. Every example in docs/ui.md (see `spec.rs`), each on a fresh chip.
//! 6. Twenty thousand ticks of random clicks and holds.
//! 7. Both of those again with an overflow landing on every instruction the
//!    main loop reaches (`Sim::land`), each as far as the first run kept
//!    reaching new code, and a little past (`both_ways`).
//!
//! All but the first two drive `ts26::torch` alongside the firmware and check
//! the pins show the library's output on every tick, the fourth until the
//! oscillator stops. `probe` separately runs examples/link.rs, which has real
//! statics.

use std::iter;
use std::ops::Range;
use std::thread;

use ts26::TICKS_PER_SECOND;
use ts26::board;
use ts26::ramp::{DSM_BITS, DSM_FLOOR, Output, PWM_TOP};
use ts26::sensors::Channel;
use ts26::testing::{self, Action, Example, Rig, Sample, Scene};
use ts26::watchdog::{CLOCK_HZ, Feeder, TICK_DEADLINE_CYCLES};

use crate::avr::{Bus, Cpu, I, VECTOR_WORDS, is_two_words, ldi};
use crate::crc;
use crate::elf::{DATA_SPACE, Elf, SRAM};
use crate::layout::{jmp_target, word};
use crate::spec;
use crate::switching::RandomSwitching;
use crate::updi::map;
use crate::world::{CALIBRATION, FUSE_NAMES, World};

// The registers the chip models, as data-space addresses, and their bits.
// They're generated from the ATDF (src/regs.rs), so they only move when the
// ATDF does, never with the firmware. Only the ones the ATDF names: anything
// hand-written in src/regs.rs is the firmware's choice, and stays out.
use ts26::regs::{
    self, ADC_ENABLE, ADC_MUXPOS_AIN1, ADC_MUXPOS_TEMPSENSE, ADC_PRESC, ADC_REFSEL,
    ADC_REFSEL_INTREF, ADC_RESRDY, ADC_SAMPCAP, ADC_SAMPLEN, ADC_SAMPNUM, ADC_SAMPNUM_ACC4,
    ADC_STCONV, CRCSCAN_BUSY, CRCSCAN_ENABLE, CRCSCAN_OK, PORT_INVEN, PORT_ISC, PORT_PULLUPEN,
    RSTCTRL_PORF, RTC_CTRLBUSY, RTC_PI, RTC_PITEN, SLPCTRL_SEN, TCA_CMP0EN, TCA_OVF,
    VREF_ADC0REFEN, VREF_ADC0REFSEL, WDT_LOCK, WDT_PERIOD, WDT_SYNCBUSY, WDT_WINDOW, addr as reg,
};

/// One past the last byte of TCA0's 16-bit registers: CMP2BUF's high byte.
const TCA0_WIDE_END: usize = reg::TCA0_CMP2BUF + 2;

const CCP_IOREG: u8 = regs::CPU_CCP_IOREG;
/// CLKCTRL.MCLKCTRLB: PDIV 2X with PEN, 20 MHz down to 10.
const CLOCK_10MHZ: u8 = regs::CLKCTRL_PDIV_2X | regs::CLKCTRL_PEN;
const ISC_INTDISABLE: u8 = regs::PORT_ISC_INTDISABLE;
const ISC_BOTHEDGES: u8 = regs::PORT_ISC_BOTHEDGES;
const ISC_RISING: u8 = regs::PORT_ISC_RISING;
const ISC_FALLING: u8 = regs::PORT_ISC_FALLING;
const ISC_INPUT_DISABLE: u8 = regs::PORT_ISC_INPUT_DISABLE;
const ISC_LEVEL: u8 = regs::PORT_ISC_LEVEL;
/// TCA0.CTRLA: CLKSEL DIV1 with ENABLE.
const TCA_RUNNING: u8 = regs::TCA_CLKSEL_DIV1 | regs::TCA_ENABLE;
/// TCA0.CTRLB.WGMODE = DSBOTTOM.
const TCA_DSBOTTOM: u8 = regs::TCA_WGMODE_DSBOTTOM;
const SLEEP_IDLE: u8 = regs::SLPCTRL_SMODE_IDLE;
const SLEEP_POWER_DOWN: u8 = regs::SLPCTRL_SMODE_PDOWN;

const VECTOR_PORTA: u8 = regs::VECTOR_PORTA_PORT as u8;
const VECTOR_PORTB: u8 = regs::VECTOR_PORTB_PORT as u8;
const VECTOR_PORTC: u8 = regs::VECTOR_PORTC_PORT as u8;
const VECTOR_RTC_PIT: u8 = regs::VECTOR_RTC_PIT as u8;
const VECTOR_TCA0_OVF: u8 = regs::VECTOR_TCA0_OVF as u8;
/// Reset plus every interrupt.
const VECTORS: usize = regs::VECTORS;

const VREF_1V1: u8 = regs::VREF_ADC0REFSEL_1V1;
/// WDT.CTRLA.PERIOD = 8KCLK, the longest. Anything over is reserved.
const WDT_PERIOD_MAX: u8 = regs::WDT_PERIOD_8KCLK;
/// CRCSCAN.CTRLB: SRC FLASH with MODE PRIORITY, both 0.
const CRCSCAN_FLASH_PRIORITY: u8 = regs::CRCSCAN_SRC_FLASH | regs::CRCSCAN_MODE_PRIORITY;
const ADC_DUTY25: u8 = regs::ADC_DUTYCYC_DUTY25;

/// CPU cycles a second, from `hw::init`'s clock setup.
const CLK_PER: u32 = 10_000_000;
// The library sizes `TICK_DEADLINE_CYCLES` for the same clock, and
// `clocked_as_modelled` holds the firmware to it.
const _: () = assert!(CLOCK_HZ == CLK_PER);
/// Reads of WDT.STATUS that report SYNCBUSY after a write to CTRLA, like
/// `PIT_SYNC_READS`.
const WDT_SYNC_READS: u8 = 2;
/// WDT clocks in a tick: the PIT divides the same oscillator by 512, and the
/// WDT by 32.
const WDT_CLOCKS_PER_TICK: u32 = 16;
/// The scan starts three cycles after ENABLE, and the CPU keeps running until
/// then (§27.3.1). No instruction is quicker than a cycle, so at most this many
/// instructions see STATUS from before the scan.
const CRC_START_INSTRUCTIONS: u64 = 3;
/// Reads of STATUS that show BUSY once the scan's started. In priority mode the
/// scan stalls the CPU until it's done, so a real chip may never show it; this
/// makes sure the firmware waits if it does.
const CRC_BUSY_READS: u8 = 2;
/// Instructions a conversion takes on the simulated chip. Four accumulated
/// conversions are about 165 ADC clocks, at 16 CPU cycles each, and that's well
/// inside a tick, so firmware that reads the result straight after starting a
/// conversion finds it isn't ready.
const CONVERSION_INSTRUCTIONS: u64 = 165 * 16;
/// What the first conversion after a change of input reads. The datasheet
/// doesn't promise it's wrong, but Anduril finds it unstable, so the model makes
/// it as wrong as it gets.
const UNSETTLED: u16 = 0;

/// Where flash appears in data space, and how much of it there is.
const MAPPED: u16 = map::FLASH;
const FLASH_SIZE: usize = map::FLASH_SIZE;

/// SRAM's contents before startup. Real SRAM powers up holding anything; a
/// fixed pattern makes a byte that should have been written stand out.
const JUNK: u8 = 0xA5;

// The pins, from `ts26::board`, which src/avr/hw.rs uses too.
const SWITCH: u8 = 1 << board::SWITCH; // PA5, active low
const PWM: u8 = 1 << board::PWM; // PB0, the brightness
const ENABLE: u8 = 1 << board::ENABLE; // PB2, the boost converter
const SWITCH_PIN: usize = board::SWITCH as usize;
const ENABLE_PIN: usize = board::ENABLE as usize;
const UPDI_PIN: usize = board::UPDI as usize;
const BATTERY_PIN: usize = board::BATTERY as usize;
/// PC1 to PC3: blue, green, red.
const AUX: u8 = board::AUX_PINS;

/// Instructions startup may take before reaching `main`.
const BOOT_BUDGET: u64 = 100_000;
/// CPU cycles in a 64 Hz tick at 10 MHz.
const TICK_CYCLES: u64 = CLK_PER as u64 / TICKS_PER_SECOND as u64;
const _: () = assert!(TICK_CYCLES * TICKS_PER_SECOND as u64 == CLK_PER as u64);
/// Instructions one pass of the main loop may take, handlers included. No
/// AVRxt instruction takes more than 4 cycles, so this many always fit.
const TICK_BUDGET: u64 = TICK_CYCLES / 4;
/// Clocks in a dual-slope PWM cycle at `PWM_TOP`.
const PWM_CYCLE: u64 = 2 * PWM_TOP as u64;
/// Clocks the CPU spends getting into a handler on a 16 KB part and back out
/// of it (§13.3.2.3, table 13-1: six in, four for `reti`).
const INTERRUPT_CYCLES: u64 = 10;
/// Instructions a handler may take, by the same sum.
const HANDLER_BUDGET: u64 = (PWM_CYCLE - INTERRUPT_CYCLES) / 4;
/// BOTTOMs in a row the live compare can be 0 with the boost converter on. At
/// the lowest level the delta-sigma only carries into the duty once every
/// 2^DSM_BITS / DSM_FLOOR cycles, so a duty of 0 for one cycle is normal there.
const ZERO_DUTY_CYCLES: u32 = (1 << DSM_BITS) / DSM_FLOOR as u32 + 1;
/// How many BOTTOMs `Tca::recent` covers: a whole turn of the delta-sigma.
const DSM_WINDOW: usize = 1 << DSM_BITS;
/// How far the live compares of a whole turn can sum from the level's `dsm`.
const DSM_SLACK: u32 = 2;

/// How many reads of RTC.PITSTATUS report busy after reset or a write to
/// PITCTRLA. The real chip is busy for a couple of 32 kHz clocks while it
/// syncs; any non-zero number makes the firmware wait.
const PIT_SYNC_READS: u8 = 2;

#[derive(Clone, Copy, Default)]
pub struct Report {
    /// The most stack the firmware used, in bytes.
    pub stack: u16,
    /// The most instructions one pass of the main loop took, from the runs
    /// without overflows landed on purpose. Every pass still has to fit the
    /// budget with them (`Sim::tick`).
    pub loop_instructions: u64,
    /// What that's measured against.
    pub loop_budget: u64,
    /// The most instructions one pass took that started off resting and ended
    /// in power-down, which is what the chip stays awake for, 64 times a
    /// second, when it's off.
    pub off_instructions: u64,
    /// The most instructions one run of the TCA0 OVF handler took.
    pub handler_instructions: u64,
    /// What that's measured against.
    pub handler_budget: u64,
    /// Overflows `Sim::land` put on an instruction of the main loop.
    pub landed: u32,
    /// Handlers that landed with a TCA0 16-bit access half done. The firmware
    /// holds interrupts off around those, so this should stay at 0.
    pub mid_access: u32,
    /// The last tick the main loop reached an instruction for the first time
    /// where `Sim::land` could put an overflow.
    last_new: u32,
}

impl Report {
    /// Adds in another run's figures.
    fn merge(&mut self, run: &Self) {
        self.stack = self.stack.max(run.stack);
        self.loop_instructions = self.loop_instructions.max(run.loop_instructions);
        self.off_instructions = self.off_instructions.max(run.off_instructions);
        self.handler_instructions = self.handler_instructions.max(run.handler_instructions);
        self.landed += run.landed;
        self.mid_access += run.mid_access;
    }

    fn take(&mut self, sim: &Sim) {
        self.stack = self.stack.max(sim.stack());
        self.handler_instructions = self.handler_instructions.max(sim.longest_handler);
        self.landed += sim.overflows_landed;
        self.mid_access += sim.mid_access;
        self.last_new = self.last_new.max(sim.last_new);
    }
}

/// The symbols every program here has, as data-space addresses or word
/// addresses as each one's used.
struct Linked {
    main: u32,
    unhandled: u32,
    layout: Layout,
}

impl Linked {
    fn of(elf: &Elf) -> Result<Self, String> {
        let symbol = |name| symbol(elf, name);
        let sram = |name| -> Result<u16, String> { Ok((symbol(name)? - DATA_SPACE) as u16) };
        Ok(Self {
            main: symbol("main")? / 2,
            unhandled: symbol("__unhandled_interrupt")?,
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

pub fn check(elf: &Elf) -> Result<Report, String> {
    let linked = Linked::of(elf)?;
    let image = elf.flash_image();
    // What `cargo xtask hex` and `flash` write.
    let signed = crc::sign(&image)?;
    let reset = symbol(elf, "__reset")?;

    // A chip that's just had its cell put in, run to the end of startup.
    let power_on = || -> Result<Sim, String> {
        let mut sim = Sim::new(signed.clone(), linked.layout.bss.end, linked.unhandled)?;
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
        linked.unhandled,
    )?)
    .map_err(|e| format!("starting with PB2 high: {e}"))?;
    boot_order(Sim::new(
        signed.clone(),
        linked.layout.bss.end,
        linked.unhandled,
    )?)
    .map_err(|e| format!("start-up order: {e}"))?;
    let mut clocks = Report::default();
    slow_ticks(power_on()?, &mut clocks).map_err(|e| format!("with the slowest ticks: {e}"))?;
    oscillator_stops(power_on()?, &mut clocks)
        .map_err(|e| format!("with the 32 kHz oscillator stopped: {e}"))?;

    let examples = spec::examples(spec::UI_MD)?;
    // Every example, then the random switching, each with and without
    // overflows landing mid-pass. Each is its own chip, so they run side by
    // side: the long ones take minutes of simulated time.
    let runs = examples.iter().map(Some).chain([None]);
    let power_on = &power_on;
    let reports: Vec<Result<Report, String>> = thread::scope(|scope| {
        let runs: Vec<_> = runs
            .map(|example| scope.spawn(move || both_ways(power_on, example)))
            .collect();
        runs.into_iter()
            .map(|run| run.join().unwrap_or_else(|_| Err("a run panicked".into())))
            .collect()
    });

    let mut report = Report {
        loop_budget: TICK_BUDGET,
        handler_budget: HANDLER_BUDGET,
        ..Report::default()
    };
    report.merge(&clocks);
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

/// Ticks a run with overflows landing goes on for after the last tick the run
/// without them reached new code, so whatever a late landing broke has time
/// to show.
const LANDING_MARGIN: u32 = 2 * TICKS_PER_SECOND as u32;

/// An example, or the random switching with `None`, run as it is and then
/// with overflows landing mid-pass. The second only lands on the first
/// time each instruction's reached, so once the first run's reached all it
/// will, the second has nothing left to land on, and the rest of it would
/// repeat the first: it stops `LANDING_MARGIN` after.
fn both_ways(
    power_on: &impl Fn() -> Result<Sim, String>,
    example: Option<&Example>,
) -> Result<Report, String> {
    let mut report = run(power_on, example, Landing::Watch)?;
    let until = report.last_new + LANDING_MARGIN;
    report.merge(&run(power_on, example, Landing::Until(until))?);
    Ok(report)
}

/// One run on a fresh chip: an example from docs/ui.md, or the random switching
/// with `None`. `Landing::Until` puts an overflow on every instruction it
/// reaches (`Sim::land`), which is the race between the delta-sigma handler
/// and the main loop.
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
            ui_example(sim, example, &mut report).map_err(|e| format!("{}: {e}", example.row))
        }
        None => against_the_library(sim, &mut report).map_err(|e| format!("random switching: {e}")),
    };
    match (result, landing) {
        (Ok(()), _) => Ok(report),
        (Err(e), true) => Err(format!("with overflows mid-pass: {e}")),
        (Err(e), false) => Err(e),
    }
}

/// Ticks of random switching to compare against the library: about five
/// minutes of a torch being clicked and held.
const RANDOM_TICKS: usize = 20_000;

/// Runs one of docs/ui.md's examples on the firmware and the library in step,
/// then checks where the library ended up against the example.
fn ui_example(mut sim: Sim, example: &Example, report: &mut Report) -> Result<(), String> {
    // Cut short, it never gets to the end. The level is the library's alone,
    // so the run that did get there has checked it.
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

/// Switches the firmware and the library the same random way. The host tests
/// trust the library; this checks the AVR build of it, and `hw.rs`'s pins, do
/// what it says.
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

/// A click from off, then `ticks` more with the switch left alone, in step
/// with the library, which has to leave the light on.
fn lit(sim: &mut Sim, ticks: u16, report: &mut Report) -> Result<(), String> {
    let script = [Action::Clicks(1), Action::Wait(ticks)];
    in_step(sim, testing::samples(script), report)?;
    if !sim.chip.enabled_high() {
        return Err("a click from off left PB2 low, so there's no light to check".into());
    }
    Ok(())
}

/// The slowest tick oscillators inside the datasheet's limits give, in main
/// clocks: the 32 kHz one 30% slow, and the 20 MHz one 5% fast, so more of
/// its clocks fit in the time (tables 36-13 and 36-12, both over the full
/// operating range, from characterisation).
const SLOWEST_TICK_CYCLES: u64 = TICK_CYCLES * 105 / 70;
/// Main clocks `hw::wait_for_tick` waits for a tick, with the light on, before
/// it gives up on the 32 kHz oscillator.
const DEADLINE_CYCLES: u64 = TICK_DEADLINE_CYCLES as u64 * PWM_CYCLE;
// The slowest tick this chip can be given is well inside the deadline, so only
// a run that stops the oscillator can trip it.
const _: () = assert!(4 * SLOWEST_TICK_CYCLES < DEADLINE_CYCLES);

/// Two seconds lit with every tick as slow as the oscillators' limits allow.
/// `hw::wait_for_tick`'s deadline mustn't trip: that stops the chip for good,
/// which `Sim::tick` fails. Only the PIT's period changes. The model's
/// watchdog counts PIT periods, so it's none the wiser.
fn slow_ticks(mut sim: Sim, report: &mut Report) -> Result<(), String> {
    sim.chip.tick_cycles = SLOWEST_TICK_CYCLES;
    lit(&mut sim, 2 * TICKS_PER_SECOND, report)
}

/// Steps `hw::halt` may take from dropping PB2 to its `sleep`: instructions,
/// or waits if it's gone to sleep some other way.
const HALT_BUDGET: u64 = 100;
/// PIT periods' worth of time to watch a chip that's stopped for good.
const DOWN_PERIODS: u64 = 4 * TICKS_PER_SECOND as u64;

/// The 32 kHz oscillator stops with the light on, and takes the tick and the
/// watchdog with it. `hw::wait_for_tick`'s deadline is all that's left, and
/// `main` answers it with `hw::halt`. This checks PB2 drops within the
/// deadline of the stop, and not before it, which a count that ran fast would
/// do. Then that the firmware gets to a `sleep` in power-down with no
/// interrupt enabled, by `ready_for_power_down`'s rules, and that nothing
/// runs and PB2 stays low for four seconds after.
///
/// The oscillator stops just as a tick lands, so the firmware has one more
/// pass, and the deadline counts from the wait that ends it.
///
/// A tick that goes missing with the oscillator still running ends another
/// way, in the watchdog resetting the chip. That can't run here: this chip
/// has no resets, so a bite is a failed run.
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
    // `lit` took the rest. `hw::halt` is a call deeper than the loop.
    report.stack = report.stack.max(sim.stack());
    Ok(())
}

/// Feeds the same switch samples to the firmware, from just after `main`, and
/// to `ts26::torch`, and checks the pins show the library's output on every
/// tick. Returns the level it ends at, or `None` if `Landing::Until` cut it
/// short.
fn in_step(
    sim: &mut Sim,
    samples: impl IntoIterator<Item = Sample>,
    report: &mut Report,
) -> Result<Option<u8>, String> {
    // The firmware reads the fuses before its first tick, so the world it
    // powers up in is the first sample's.
    let mut samples = samples.into_iter().peekable();
    let power_on = World::from(samples.peek().map_or(Scene::NORMAL, |s| s.scene));
    sim.chip.world = power_on;
    if power_on.scene.corrupt_flash {
        // The checksum's high byte, which is never code, so the firmware
        // still runs however big it gets.
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
        // An overflow landed on purpose runs the handler on top of the pass,
        // which a real tick never sees that many of.
        if !landing {
            report.loop_instructions = report.loop_instructions.max(took);
        }
        let powered_down = sim.chip.power_downs > power_downs;
        // Only a pass that started off resting, which is what a torch that's
        // off spends every tick.
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
        was_resting = rig.torch.resting();
        // `main`'s feeder, ticked at the end of each pass as it is there. The
        // sleep that ended this pass was the firmware's answer to the state
        // the pass left behind.
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

/// What examples/link.rs's `main` leaves in `LINK_PROBE_COPY`: its
/// .rodata table, then its .data static's starting values, then a byte it
/// doesn't touch.
const PROBE_EXPECTED: [u8; 7] = [0x12, 0x34, 0x56, 0x9A, 0xBC, 0xDE, 0];

/// Runs the link probe from reset with SRAM full of junk, and checks it read
/// its const table through the mapped-flash window and its .data static from
/// where the startup code copied it.
pub fn probe(elf: &Elf) -> Result<(), String> {
    let linked = Linked::of(elf)?;
    let layout = &linked.layout;
    let rodata = elf.section(".rodata").ok_or("no .rodata section")?;
    if rodata.is_empty() || layout.data.is_empty() || layout.bss.is_empty() {
        return Err(".rodata, .data or .bss is empty, so there's nothing to check".into());
    }
    let mut sim = Sim::new(elf.flash_image(), layout.bss.end, linked.unhandled)?;
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

/// Where src/avr/rt.rs's startup copies and zeroes, as data-space addresses.
struct Layout {
    data: Range<u16>,
    bss: Range<u16>,
    /// .data's initial values, in the mapped-flash window.
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

/// Moves .data and .bss somewhere awkward, gives .data five bytes of initial
/// values, patches the addresses into the real startup code and runs it. Then
/// checks it wrote exactly those bytes and nothing either side. `reset` is the
/// byte address of `__reset`.
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

    let mut sim = Sim::new(image, moved.bss.end, linked.unhandled)?;
    sim.run_to(linked.main, BOOT_BUDGET)?;
    check_startup(&sim.chip, &moved)?;
    for edge in [moved.data.start - 1, moved.bss.end] {
        if sim.chip.sram(&(edge..edge + 1)) != [JUNK] {
            return Err(format!("wrote to {edge:#06x}, outside .data and .bss"));
        }
    }
    Ok(())
}

/// Rewrites the `ldi`s in src/avr/rt.rs's startup that load the .data and
/// .bss addresses, after checking they're the ones expected, in order.
fn patch_startup(
    image: &mut [u8],
    reset: usize,
    linked: &Layout,
    moved: &Layout,
) -> Result<(), String> {
    // (register, byte) for each `ldi`, as rt.rs loads them: X = __data_start,
    // Z = __data_load, r25:r24 = __data_end, X = __bss_start, r25:r24 = __bss_end.
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

/// Runs `sim` from the reset vector with PORTB as a lit torch has it, PB2 an
/// output and high, and checks the first register the firmware writes clears
/// PB2 in VPORTB.OUT. That's `hw::hold_enable_low`'s job, ahead of the CRC
/// scan and everything else.
///
/// A stray jump to the vector would find the port like that, since nothing
/// short of a reset clears OUT. This isn't a model of one, though. Only PORTB
/// starts dirty: the rest of the chip, SP and SREG included, is as a reset
/// leaves it. It shows what the firmware's first write does to a pin it finds
/// high, and that's all.
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

/// Runs `sim` from reset to the first wait for a tick, and checks `main` got
/// there in the order it has to. The reset flags are cleared before the
/// watchdog's on. The CRC scan and the fuse read come before the chip's set up
/// at all. The PIT's on before the watchdog. `found_lit` checks what comes
/// before all of these.
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

/// A bit for each vector whose slot in `image`'s table is a `jmp` to
/// somewhere other than `unhandled` (a word address). layout.rs checks every
/// slot is a `jmp`.
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

/// What a handler found when it landed, with its return address pushed.
struct Landed {
    r: [u8; 32],
    sreg: u8,
    sp: u16,
}

struct Sim {
    cpu: Cpu,
    chip: Chip,
    /// SP below this means a push landed in .bss.
    stack_floor: u16,
    lowest_sp: u16,
    /// Instructions run, which unlike `Chip::now` stops while asleep.
    executed: u64,
    /// `__unhandled_interrupt`, as a word address.
    unhandled: u32,
    /// A bit per vector whose slot doesn't jump there.
    handled: u32,
    /// What the running handler has to give back.
    landed: Option<Landed>,
    /// Instructions the running handler has taken.
    handler_steps: u64,
    /// The most any TCA0 OVF handler has taken.
    longest_handler: u64,
    /// What `land` does.
    landing: Landing,
    /// A bit per word of flash, for the instructions `land` has seen.
    reached: Vec<bool>,
    /// The tick `in_step` is on.
    tick_no: u32,
    /// The last tick `land` saw an instruction for the first time.
    last_new: u32,
    /// Overflows landed so far.
    overflows_landed: u32,
    /// Handlers so far that landed with a TCA0 16-bit access half done.
    mid_access: u32,
}

/// What `Sim::land` does the first time the main loop reaches an instruction
/// where an overflow could land.
#[derive(Clone, Copy)]
enum Landing {
    /// Nothing.
    Off,
    /// Notes the tick, in `Sim::last_new`.
    Watch,
    /// Lands an overflow there, until `in_step` stops after this tick.
    Until(u32),
}

impl Sim {
    fn new(image: Vec<u8>, bss_end: u16, unhandled: u32) -> Result<Self, String> {
        if image.len() > FLASH_SIZE {
            return Err(format!("{} bytes won't fit in flash", image.len()));
        }
        let cpu = Cpu::new(&SRAM);
        let unhandled = unhandled / 2;
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
            landed: None,
            handler_steps: 0,
            longest_handler: 0,
            landing: Landing::Off,
            reached: vec![false; FLASH_SIZE / 2],
            tick_no: 0,
            last_new: 0,
            overflows_landed: 0,
            mid_access: 0,
        })
    }

    /// Before the first time the main loop reaches each instruction, an
    /// overflow goes up, so the handler lands there or, with interrupts held
    /// off, at the first place it can. Left alone a 510-clock cycle only lands
    /// on the instructions its period happens to meet.
    ///
    /// It isn't a real BOTTOM, so the live compare doesn't move, and the handler
    /// steps the delta-sigma once more than the PWM does, and can overwrite a
    /// carry in CMP0BUF before BOTTOM takes it, which the real handler, once a
    /// cycle, can't. So neither the sum in `Chip::shows` nor the count of
    /// zero-duty cycles can hold across one, and both start again.
    ///
    /// `Landing::Watch` only notes when each instruction's first reached, so
    /// `both_ways` knows how far a landing run has new code to land on.
    fn land(&mut self) {
        if let Landing::Off = self.landing {
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
                chip.tca.settled = 0;
                chip.tca.zero_run = 0;
                self.overflows_landed += 1;
            }
        }
    }

    /// The most stack used so far, in bytes.
    fn stack(&self) -> u16 {
        SRAM.end - 1 - self.lowest_sp
    }

    fn step(&mut self) -> Result<(), String> {
        let pc = self.cpu.pc;
        let awake = !self.cpu.asleep;
        let handling = self.cpu.handling;
        self.chip.in_handler = handling.is_some();
        self.cpu.step(&mut self.chip)?;
        self.watch_handler(handling)?;
        if awake {
            self.executed += 1;
            self.chip.elapse()?;
        }
        self.lowest_sp = self.lowest_sp.min(self.cpu.sp);
        if self.cpu.sp < self.stack_floor {
            return Err(format!("{:#06x}: the stack ran into .bss", pc * 2));
        }
        // The real chip would `reti` straight back into the main loop with
        // the flag still up, and take the interrupt again, forever. Better to
        // catch it as soon as one's enabled, before it ever fires.
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
        {
            return Err(format!(
                "takes {}, whose vector goes to __unhandled_interrupt",
                vector_name(vector)
            ));
        }
        self.land();
        Ok(())
    }

    /// Takes note of a handler starting or finishing. `before` is what the
    /// CPU was handling before the step just run.
    fn watch_handler(&mut self, before: Option<u8>) -> Result<(), String> {
        match (before, self.cpu.handling) {
            (None, Some(_)) => {
                self.chip.interrupted = self.chip.tca.temp.map(|(low, _)| low);
                self.mid_access += u32::from(self.chip.interrupted.is_some());
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

    /// A handler lands between any two instructions of the main loop, so
    /// anything it changes, the compiled code around it sees change at random.
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

    /// Runs until PC reaches `target`, a word address.
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

    /// One pass of the main loop with the switch as given: runs until the
    /// next tick. Returns how many instructions the pass took, which stops
    /// when the firmware starts waiting for that tick, by polling the PIT's
    /// flag or by going to sleep for the last time. A firmware that stops for
    /// good (`hw::halt`) with ticks still coming has failed.
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

/// The watchdog, as the firmware can see it. Its settings are in
/// `io[WDT_CTRLA]`. It's timed in PIT periods, not instructions, since the two
/// share an oscillator: a period is always 16 WDT clocks.
#[derive(Default)]
struct Wdt {
    /// Ticks since the last `wdr`, or since CTRLA was written.
    ticks: u32,
    /// Whether a `wdr` has started window mode's closed windows.
    windowed: bool,
    /// Reads of WDT.STATUS that still report SYNCBUSY after a CTRLA write.
    busy: u8,
}

/// WDT clocks in a WDT.CTRLA PERIOD or WINDOW setting, 0 for off.
fn wdt_clocks(setting: u8) -> u32 {
    match setting {
        0 => 0,
        n => 8 << (n - 1),
    }
}

/// When each step of `main`'s start-up first happened, in `Chip::now`, for
/// `boot_order`.
#[derive(Default)]
struct Boot {
    /// The first RSTCTRL.RSTFR write, `hw::take_reset_flags`.
    flags_cleared: Option<u64>,
    /// The first CRCSCAN.CTRLA write, `hw::flash_ok`.
    flash_checked: Option<u64>,
    /// The first fuse read, `hw::fuses`.
    fuses_read: Option<u64>,
    /// The first write that isn't one of those or PORTB's, so the start of
    /// `hw::init`.
    set_up: Option<u64>,
    /// The first RTC.PITCTRLA write with PITEN.
    pit_on: Option<u64>,
    /// The first WDT.CTRLA write, `hw::start_watchdog`.
    watchdog_on: Option<u64>,
}

impl Boot {
    /// Notes a register write at `now`.
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
        let at = |when: Option<u64>, what: &str| when.ok_or(format!("never {what}"));
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

/// CRCSCAN's state beyond its plain registers.
struct CrcScan {
    /// The instruction that last wrote ENABLE, and what STATUS read just
    /// before it.
    started: Option<(u64, u8)>,
    /// Reads of STATUS still to show BUSY.
    busy_reads: u8,
    /// STATUS once the scan's done. OK from reset (§27.5.3), before any scan,
    /// which is why the firmware has to wait for its own.
    status: u8,
}

/// ADC0's state beyond its plain registers.
#[derive(Default)]
struct Adc {
    /// The input the last conversion used, to spot a switch.
    last_setup: Option<u8>,
    /// The result of the last conversion started.
    result: u16,
    /// When that conversion finishes, while its result's unread.
    ready_at: Option<u64>,
    /// The high byte of RES, latched into ADC0.TEMP by reading the low byte.
    temp: Option<u8>,
}

/// TCA0, beyond the registers in `Chip::io`.
struct Tca {
    /// PER, live.
    per: u16,
    perbuf: u16,
    /// CTRLF.PERBV.
    perbv: bool,
    /// CMP0, live.
    cmp0: u16,
    cmp0buf: u16,
    /// CTRLF.CMP0BV.
    cmp0bv: bool,
    /// The TEMP byte, and which register's access it's holding. There's one
    /// for all the 16-bit registers (§20.5.13).
    temp: Option<(usize, u8)>,
    /// When the counter next reaches BOTTOM.
    next_bottom: u64,
    /// INTFLAGS.OVF.
    ovf: bool,
    /// BOTTOMs in a row with PB2 high and the live compare at 0.
    zero_run: u32,
    /// The live compare at the last `DSM_WINDOW` BOTTOMs, oldest at `at`.
    recent: [u16; DSM_WINDOW],
    at: usize,
    /// BOTTOMs since PB2 last moved, up to `DSM_WINDOW`.
    settled: usize,
}

struct Chip {
    flash: Vec<u8>,
    sram: Vec<u8>,
    /// Plain storage for the registers that are only storage.
    io: Vec<u8>,
    /// The time, counted an instruction at a time while the CPU's awake, and
    /// jumping on to the next event while it sleeps.
    now: u64,
    /// When something next happens, so most instructions needn't look.
    next_event: u64,
    /// The last instruction a protected register still takes a write on.
    unlocked_until: Option<u64>,
    /// Whether the CCP period's still holding interrupts off. The key's first
    /// store to the bus or `sleep` ends it (§8.5.7.1).
    ccp_hold: bool,
    pressed: bool,
    pit_busy: u8,
    /// RTC.PITINTFLAGS.PI.
    pit_flag: bool,
    /// PORTA.INTFLAGS.
    porta_flags: u8,
    /// When the PIT's current period ends.
    next_pit: u64,
    /// Main clocks in a PIT period, which is the 32 kHz oscillator's speed
    /// against the 20 MHz one's. `TICK_CYCLES` unless a run slows it.
    tick_cycles: u64,
    /// The 32 kHz oscillator has stopped. The PIT and the WDT both count it
    /// (§23.12.7's reset value, §19.3.2), so neither does anything more: a
    /// period is only that much time going by. The firmware writes neither
    /// after start-up, so a sync left waiting on the oscillator isn't modelled.
    osc32k_stopped: bool,
    /// The last `sleep` ran with no interrupt enabled, so only a reset wakes
    /// the chip (§11.3.1), and this one has no resets.
    down: bool,
    /// PIT periods so far with the PIT on.
    ticks: u32,
    /// Whether the firmware's started waiting for a tick since this was
    /// cleared.
    tick_polled: bool,
    tca: Tca,
    wdt: Wdt,
    crc: CrcScan,
    adc: Adc,
    /// RSTCTRL.RSTFR.
    rstfr: u8,
    /// What the ADC measures, and the fuses.
    world: World,
    /// A bit per vector whose interrupt is enabled.
    enabled: u32,
    /// Whether the instruction running is in a handler.
    in_handler: bool,
    /// The low byte of the TCA0 register the running handler caught halfway
    /// through an access, which is what it mustn't disturb.
    interrupted: Option<usize>,
    /// Times `sleep` has run in power-down.
    power_downs: u32,
    /// Writes to anything but SRAM so far.
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
            in_handler: false,
            interrupted: None,
            power_downs: 0,
            register_writes: 0,
            boot: Boot::default(),
        }
    }

    fn sram(&self, range: &Range<u16>) -> &[u8] {
        &self.sram[usize::from(range.start - SRAM.start)..usize::from(range.end - SRAM.start)]
    }

    /// PB2, as the boost converter sees it.
    fn enabled_high(&self) -> bool {
        self.io[reg::VPORTB_OUT] & self.io[reg::VPORTB_DIR] & ENABLE != 0
    }

    /// The aux pins that are lit, as a PORTC mask. An output driven high is
    /// bright; an input with its pull-up on glows dimly; an output driven low
    /// is dark.
    fn aux(&self) -> u8 {
        let dir = self.io[reg::VPORTC_DIR];
        let pulled_up = (0..8).fold(0, |bits, pin| {
            let on = self.io[reg::PORTC_PIN0CTRL + pin] & PORT_PULLUPEN != 0;
            bits | u8::from(on) << pin
        });
        (self.io[reg::VPORTC_OUT] & dir | pulled_up & !dir) & AUX
    }

    /// Refuses a chip `hw::init` left on the wrong clock or PWM. Writes of
    /// anything else are refused already, so this is what catches a write
    /// that never happened.
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

    /// The duty PB0 really gets: the live compare, if TCA0 has the pin.
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

    /// Spends the CCP key on a write to protected register `a`, or refuses
    /// the write if there's no key to spend.
    fn take_protected_write(&mut self, a: usize) -> Result<(), String> {
        if !self.unlocked_until.is_some_and(|until| self.now <= until) {
            return Err(format!(
                "writes {} without the CCP key in the four instructions before, \
                 and the chip ignores that write",
                name(a)
            ));
        }
        self.unlocked_until = None;
        Ok(())
    }

    /// Moves time on to `to`, and runs whatever falls due on the way. This is
    /// the one place simulated time moves.
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

    /// The open window in WDT clocks, which is the whole timeout in normal
    /// mode, or 0 with the WDT off.
    fn wdt_open(&self) -> u32 {
        wdt_clocks(self.io[reg::WDT_CTRLA] & WDT_PERIOD)
    }

    /// The closed window in WDT clocks, 0 in normal mode or before the first
    /// `wdr` starts window mode (§19.3.3.2).
    fn wdt_closed(&self) -> u32 {
        if self.wdt.windowed {
            wdt_clocks((self.io[reg::WDT_CTRLA] & WDT_WINDOW) >> 4)
        } else {
            0
        }
    }

    /// A PIT period's worth of WDT clocks, while the watchdog's on. It runs
    /// in every sleep mode (§19.3.4), so this counts asleep too. Nothing here
    /// resets the chip: running out is the firmware failing.
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

    /// CRCSCAN.STATUS as a read sees it now.
    fn crc_status(&mut self) -> u8 {
        match self.crc.started {
            Some((at, before)) if self.now <= at + CRC_START_INSTRUCTIONS => before,
            Some(_) if self.crc.busy_reads > 0 => {
                self.crc.busy_reads -= 1;
                CRCSCAN_BUSY
            }
            _ => self.crc.status,
        }
    }

    /// Starts a whole-flash scan: CRC-16-CCITT over every byte of flash, erased
    /// bytes reading 0xFF, passing if it comes to 0.
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

    /// Starts a conversion, if the ADC's set up for one of the two measurements
    /// the firmware makes, and works out what it'll read.
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
                // §30.3.1.1: the digital input buffer should be off on the
                // pin the ADC reads. That and nothing else, so no pull-up.
                // The reading is the world's either way, since the advice
                // is for a pin that's still being converted (§16.3.2.2).
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
                // Section 30.3.2.6: SAMPCAP, and INITDLY and SAMPLEN each at
                // least 32 µs of ADC clock.
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

    /// One instruction's worth of time.
    fn elapse(&mut self) -> Result<(), String> {
        self.advance_to(self.now + 1)
    }

    /// The counter reaches BOTTOM: the overflow flag goes up, and the UPDATE
    /// copies whichever buffers have been written (table 20-6).
    fn bottom(&mut self) -> Result<(), String> {
        let tca = &mut self.tca;
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

    /// The live compares of the last whole turn of the delta-sigma, added up,
    /// which comes to the `dsm` it was given. `None` if PB2 hasn't been steady
    /// for that long.
    fn dsm_sum(&self) -> Option<u32> {
        (self.tca.settled == DSM_WINDOW)
            .then(|| self.tca.recent.iter().map(|&c| u32::from(c)).sum())
    }

    /// Whether the pins are doing what `output` asks.
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

    /// INVEN on PA5 would flip both VPORTA.IN and which edge is which
    /// (§16.3.2.2), and the model does neither, so it refuses it.
    fn switch_ctrl(&self) -> u8 {
        self.io[reg::PORTA_PIN0CTRL + SWITCH_PIN]
    }

    /// Presses or releases the switch on PA5. A change sets PA5's flag in
    /// PORTA.INTFLAGS if its ISC asks for that edge (§16.5.10). INTDISABLE,
    /// which is where the firmware leaves it, asks for none.
    ///
    /// `Sim::tick` only moves the switch between ticks, so an edge that's
    /// asked for always raises its flag. A press that's over before the chip
    /// has woken from power-down can wake it with no flag at all (table
    /// 16-3's note 1), and that isn't modelled.
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

    /// A bit per vector, a port's being set when `senses` is true of any of
    /// its pins' number and ISC, and the PIT's and TCA0's when `timers` says
    /// and their interrupts are on.
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

    /// What can wake the chip from power-down. TCA0 stops, and only pins 2
    /// and 6 are fully asynchronous: the rest wake it on BOTHEDGES or LEVEL
    /// and no other sense (table 16-3).
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

    /// What has to be true before `sleep` in power-down: the boost converter
    /// off, and nothing left to come back on when TCA0 starts again.
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

    /// A write to PORTx.PINnCTRL, refusing what the board can't take.
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

    /// A write to VPORTB.OUT or DIR, watching what PB2 does.
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

    /// The firmware has finished writing a 16-bit TCA0 register.
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

    /// Refuses an access to `a` by a handler that caught the main loop
    /// halfway through a TCA0 16-bit register, since they all share TEMP.
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

/// Registers that are plain storage as far as the firmware's concerned.
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

/// The registers that decide what a conversion measures. Changing one
/// mid-conversion is a mistake the firmware shouldn't make.
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

/// The fuses and the lock byte after them.
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

    /// Lowest vector first, the default static priority (§13.3.2.4). A port's
    /// flags only go up for pins whose ISC enables them, so any flag is a
    /// request.
    fn interrupt(&self) -> Option<u8> {
        let pending = u32::from(self.porta_flags != 0) << VECTOR_PORTA
            | u32::from(self.pit_flag) << VECTOR_RTC_PIT
            | u32::from(self.tca.ovf) << VECTOR_TCA0_OVF;
        let requests = pending & self.enabled;
        (requests != 0).then(|| requests.trailing_zeros() as u8)
    }

    /// Interrupts are ignored for the four instructions after the key, and
    /// left pending (§8.7.1, §8.5.7.2). That the store or `sleep` which ends
    /// the protected change ends this too is likely, not stated.
    fn holds_interrupts(&self) -> bool {
        self.ccp_hold && self.unlocked_until.is_some_and(|until| self.now <= until)
    }

    /// With an interrupt enabled that can't wake it from the mode it's in,
    /// that's a mistake. With none enabled at all, it's the chip stopping for
    /// good: only a reset wakes it (§11.3.1). That sets `down`, which is for
    /// whoever's running the chip to judge.
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

    /// Jumps to the next thing that happens. TCA0 stops in power-down
    /// (§20.3.6 only promises idle), so its cycle waits for it there.
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
            // Erased flash past the end of the image reads as 0xFF.
            return Ok(*self.flash.get(usize::from(addr - MAPPED)).unwrap_or(&0xFF));
        }
        match a {
            // Every ISC but INPUT_DISABLE leaves the input buffer on, INTDISABLE
            // included (§16.3.2.1, §16.5.11). The pull-up is Anduril's setting
            // for the pin (docs/hardware.md). Nobody's seen whether the board
            // has one of its own, so this takes it that it hasn't.
            //
            // A read of the port samples every pin whose buffer is on, and
            // PA1 is the battery divider, which nothing should read as a
            // logic level, so its buffer has to be off by now (§30.3.1.1).
            // Its bit then stops following the pin (§16.5.11's note). What
            // it's left holding isn't known, and this has it read 1.
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
            // Reading the flag doesn't move time on.
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
                Ok(if ready { ADC_RESRDY } else { 0 })
            }
            reg::ADC0_RES => {
                // Reading the low byte latches the high byte into ADC0.TEMP
                // and clears RESRDY.
                if !self.adc.ready_at.is_some_and(|at| self.now >= at) {
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
                // A full cycle at whatever PER is, which is the slow reading.
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
            reg::TCA0_INTCTRL => {
                self.io[a] = value;
                self.update_enabled();
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
            reg::RTC_PITINTCTRL => {
                self.io[a] = value;
                self.update_enabled();
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
                // Writing a 1 clears RESRDY. It only clears the flag: a
                // conversion that's still running finishes and sets it again.
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

    /// PORTA.PIN1CTRL, PA1's settings.
    const PA1_CTRL: usize = reg::PORTA_PIN0CTRL + BATTERY_PIN;

    /// A chip with PA1 and the ADC set up as `hw::init` and `adc_channel` do.
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

    /// §30.3.1.1. The buffer being off changes nothing the ADC reads, which
    /// `the_first_conversion_after_a_change_is_junk_then_it_reads_the_world`
    /// shows, since `adc_ready` turns it off.
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
        // The temperature sensor isn't on a pin.
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

    /// A chip whose watchdog runs 32 WDT clocks closed and 32 open, two ticks
    /// each, and has synced.
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
        let signed = crc::sign(&[0x12, 0x34]).unwrap();
        let mut bad = signed.clone();
        bad[0] ^= 1;
        for (image, want) in [(signed, CRCSCAN_OK), (bad, 0)] {
            let mut chip = Chip::new(image);
            chip.write(reg::CRCSCAN_CTRLA as u16, CRCSCAN_ENABLE)
                .unwrap();
            // OK from reset, until the scan starts three cycles later.
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

    /// A chip with TCA0 set up as `hw::init` does, already past its first
    /// cycle, so PER's at `PWM_TOP`.
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
        chip.write(reg::TCA0_INTCTRL as u16, TCA_OVF).unwrap();
        chip.write(reg::VPORTB_OUT as u16, ENABLE).unwrap();
        cycles(&mut chip, u64::from(ZERO_DUTY_CYCLES)).unwrap();
        let err = cycles(&mut chip, 1).unwrap_err();
        assert!(err.contains("duty at 0"), "{err}");
    }

    #[test]
    fn dropping_the_enable_ends_a_zero_duty_run() {
        let mut chip = running();
        chip.write(reg::TCA0_INTCTRL as u16, TCA_OVF).unwrap();
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
        // A byte that isn't TCA0's is fine.
        chip.write(reg::GPIO_GPIOR0 as u16, 1).unwrap();
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
            found_lit(Sim::new(image, SRAM.start, 0)?)
        };
        let dir = out(reg::VPORTB_DIR, 16);
        let clear = out(reg::VPORTB_OUT, 17);
        assert_eq!(run([clear, dir]), Ok(()));
        let err = run([dir, clear]).unwrap_err();
        assert!(err.contains("leaves PB2 set"), "{err}");
        // Neither write, so it runs off the end of the image.
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
        chip.sleep().unwrap();
    }

    #[test]
    fn idle_with_the_light_on_is_fine_and_wakes_on_the_overflow() {
        let mut chip = running();
        chip.write(reg::TCA0_INTCTRL as u16, TCA_OVF).unwrap();
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

        // With one enabled, it's an ordinary sleep.
        chip.write(reg::TCA0_INTCTRL as u16, TCA_OVF).unwrap();
        assert_eq!(chip.sleep(), Ok(true));
        assert!(!chip.down);
    }

    /// `[sei, sleep]` with SLPCTRL set for idle and nothing enabled.
    #[test]
    fn a_pass_that_stops_for_good_fails() {
        let image = [0x9478u16, 0x9588]
            .into_iter()
            .flat_map(u16::to_le_bytes)
            .collect();
        let mut sim = Sim::new(image, SRAM.start, 0).unwrap();
        sim.chip.io[reg::SLPCTRL_CTRLA] = SLEEP_IDLE | SLPCTRL_SEN;
        let err = sim.tick(false).unwrap_err();
        assert!(err.contains("stopped for good"), "{err}");
    }

    #[test]
    fn a_stopped_oscillator_stops_the_pit_and_the_watchdog() {
        let mut chip = watched();
        chip.io[reg::RTC_PITCTRLA] = RTC_PITEN;
        chip.osc32k_stopped = true;
        // Long past the watchdog's 64 clocks.
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
        // The period under way at the change runs out as it was.
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
        chip.write(reg::TCA0_INTCTRL as u16, TCA_OVF).unwrap();
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

    /// PA5 as `hw::init` leaves it: the pull-up, and ISC at INTDISABLE. PA1
    /// too, with its input buffer off, or the read would sample the divider.
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
        // Nor is it a way to wake up.
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
    }
}
