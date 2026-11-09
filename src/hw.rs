//! The TS26 driver's pins and the ATtiny1616 registers that drive them.
//!
//! No PAC: the register constants are generated from Microchip's ATDF into
//! `src/regs.rs` at build time. The pin map is `ts26::board`'s.

#![allow(unsafe_code)]

use core::arch::asm;
use core::ptr::{read_volatile, write_volatile};

use ts26::board::{
    self, AUX_BLUE, AUX_GREEN, AUX_PINS, AUX_RED, BATTERY, ENABLE, PWM, SWITCH, UPDI,
};
use ts26::ramp::{DSM_BITS, DSM_TOP, Output, PWM_TOP};
use ts26::selftest::Fuses;
use ts26::sensors::{Channel, Convert};
use ts26::temp::Calibration;
use ts26::ui::Aux;

use crate::regs::{self, addr as reg};

/// The PORTB pins we drive. PORTB has PB0 to PB5 on the 20-pin package.
const PORTB_OUTPUTS: u8 = 1 << PWM | 1 << ENABLE;
const PORTB_PINS: u8 = 6;
/// The PORTC pins we drive. PORTC has PC0 to PC3 on the 20-pin package.
const PORTC_OUTPUTS: u8 = AUX_PINS;
const PORTC_PINS: u8 = 4;

/// TCA0.CTRLB with the light off: dual-slope, PB0 left to PORT.
const CTRLB_OFF: u8 = regs::TCA_WGMODE_DSBOTTOM;
/// TCA0.CTRLB with the light on: the same, with WO0 driving PB0.
const CTRLB_ON: u8 = regs::TCA_CMP0EN | regs::TCA_WGMODE_DSBOTTOM;

// WO2's pad is PB2, so a CTRLB with CMP2EN would hand the boost enable to
// the timer.
const _: () = assert!(ENABLE == 2 && (CTRLB_OFF | CTRLB_ON) & regs::TCA_CMP2EN == 0);

// The window `start_watchdog` sets is the one `ts26::watchdog` counts on.
// WDT.CTRLA's fields count 8 << (n - 1) WDT clocks, and these are n = 3.
const _: () = {
    use ts26::watchdog::{CLOSED_CLOCKS, OPEN_CLOCKS};
    assert!(8 << ((regs::WDT_WINDOW_32CLK >> 4) - 1) == CLOSED_CLOCKS);
    assert!(8 << (regs::WDT_PERIOD_32CLK - 1) == OPEN_CLOCKS);
};

// Every peripheral (the ports, the timers, the clock controller) exposes
// its settings as bytes at fixed addresses, in the same address space as
// SRAM. Storing a byte at `VPORTB_DIR` doesn't put it in memory, it flips
// the direction of PORTB's pins. Loading from `VPORTA_IN` reads the voltage
// on PORTA's pins right now. These two helpers are the only way the rest of
// the file does it.
//
// They're `volatile` because the compiler would otherwise treat these as
// ordinary memory, and could drop a write nobody reads back or read once
// and reuse the value in a loop. `read_volatile` and `write_volatile`
// promise each access happens, in order, exactly as written.

fn write(addr: usize, value: u8) {
    // SAFETY: every caller passes one of the `reg` constants, which are
    // memory-mapped I/O registers on this part.
    unsafe { write_volatile(addr as *mut u8, value) }
}

fn read(addr: usize) -> u8 {
    // SAFETY: as for `write`.
    unsafe { read_volatile(addr as *const u8) }
}

/// 16-bit peripheral registers go through a shared TEMP byte: write the low
/// byte first, then the high byte. Spelling it out keeps the order from
/// depending on the compiler.
///
/// The CPU only moves 8 bits at a time, so a 16-bit value takes two
/// stores. If the timer saw the halves land separately it could briefly
/// run with a new low byte and an old high byte. Instead, the low-byte
/// write is parked in TEMP, and the high-byte write copies both into the
/// register in the same clock cycle.
fn write16(addr: usize, value: u16) {
    let [lo, hi] = value.to_le_bytes();
    write(addr, lo);
    write(addr + 1, hi);
}

/// Writes a register behind configuration change protection. The CPU only
/// accepts the write within four instructions of the CCP key, so it has to
/// be assembly.
///
/// Some registers, like the clock settings, could brick a running program
/// if a stray pointer hit them, so the chip guards them. Writing the magic
/// value 0xD8 (`CPU_CCP_IOREG`) to the CCP register unlocks them for the
/// next four instructions, and interrupts are held off for that window.
/// Rust gives no guarantee about how many instructions it puts between two
/// `write` calls, so the pair is written out by hand.
fn protected_write(addr: usize, value: u8) {
    // `out` only reaches the first 64 I/O addresses, which are also the
    // first 64 of data space.
    const _: () = assert!(reg::CPU_CCP < 0x40);
    // SAFETY: writes the IOREG key to CPU.CCP, then `value` to `addr`.
    // Touches no memory Rust knows about.
    unsafe {
        asm!(
            // Unlock. `out` writes a register to an I/O address. CCP sits
            // at 0x34, low enough for `out` to reach.
            "out {ccp}, {key}",
            // Write the protected register, the very next instruction.
            // `st Z, r` stores register r at the address held in Z (the
            // r31:r30 pair). It's used here because `out` only reaches the
            // first 64 addresses and the clock registers are further up.
            "st Z, {value}",
            ccp = const reg::CPU_CCP,
            key = in(reg) regs::CPU_CCP_IOREG,
            value = in(reg) value,
            in("Z") addr as u16,
            // The block doesn't touch the stack, and neither `out` nor `st`
            // changes the status flags in SREG.
            options(nostack, preserves_flags),
        );
    }
}

/// Drives PB0 and PB2 low, the boost converter off. `main` calls this before
/// anything else, since PB2 is a high-impedance input from reset until it's
/// an output, and the CRC scan in [`flash_ok`] stalls the CPU for about 7 ms
/// before [`init`] runs. [`init`] writes DIR again.
///
/// OUT goes first, then DIR. A reset leaves OUT at zero, and this doesn't
/// count on having had one. If the CPU ever got to the reset vector some
/// other way, by a stray jump or a dip in the supply with the brown-out
/// detector off, OUT could still have PB2 set from a light that was on, and
/// DIR alone would hold the boost converter on through the scan and `init`.
/// Nobody's seen either happen. The simulator checks the order, starting
/// with PORTB dirty and the rest of the chip clean (`found_lit` in
/// xtask/src/sim.rs), and that's as far as it's been tested.
pub fn hold_enable_low() {
    write(reg::VPORTB_OUT, 0);
    write(reg::VPORTB_DIR, PORTB_OUTPUTS);
}

pub fn init() {
    // 20 MHz oscillator (the factory OSCCFG fuse) divided by 2. 10 MHz is
    // specified down to 2.7 V of VCC. VCC here comes from a regulator, not
    // the cell, and nobody knows yet how it behaves as the cell sags.
    protected_write(
        reg::CLKCTRL_MCLKCTRLB,
        regs::CLKCTRL_PDIV_2X | regs::CLKCTRL_PEN,
    );

    // Pull every pin we don't drive up, so nothing floats and leaks current.
    // PA0 is UPDI. PA1 is analogue, so it keeps its reset setting: input
    // buffer on, no pull-up to skew the divider. The outputs get no pull-up
    // either, so PB2 can't creep the boost converter on before it's an
    // output.
    //
    // PA5 is the switch, and its pull-up is what the switch pulls against.
    // ISC stays at INTDISABLE, its reset value: no interrupt, and the input
    // buffer on, so VPORTA.IN still reads the pin (DS40002204A sections
    // 16.3.2.1 and 16.5.11). `wait_for_tick` says why there's no interrupt.
    for pin in 0..8 {
        let ctrl = match pin {
            UPDI | BATTERY => continue,
            SWITCH => regs::PORT_PULLUPEN | regs::PORT_ISC_INTDISABLE,
            _ => regs::PORT_PULLUPEN,
        };
        write(reg::PORTA_PIN0CTRL + pin as usize, ctrl);
    }
    for pin in 0..PORTB_PINS {
        if PORTB_OUTPUTS & 1 << pin == 0 {
            write(reg::PORTB_PIN0CTRL + pin as usize, regs::PORT_PULLUPEN);
        }
    }
    for pin in 0..PORTC_PINS {
        if PORTC_OUTPUTS & 1 << pin == 0 {
            write(reg::PORTC_PIN0CTRL + pin as usize, regs::PORT_PULLUPEN);
        }
    }

    // PORTB's OUT is zero because `hold_enable_low` cleared it. PORTC's is as
    // reset left it, zero, and nothing in here ever writes it. So these all
    // come up low: the boost converter off, and the aux LEDs dark.
    write(reg::VPORTB_DIR, PORTB_OUTPUTS);
    write(reg::VPORTC_DIR, PORTC_OUTPUTS);

    // TCA0 stays in single mode (CTRLD.SPLITM at its reset value of 0),
    // which is the only mode PER, CMP0 and CMP0BUF mean anything in.
    // Dual-slope PWM on WO0, straight off the 10 MHz clock. The BUF
    // registers apply at the bottom of a cycle, so duty changes never
    // glitch. PER goes in directly, with the timer stopped, or the first
    // cycle would run at the reset value of 0xFFFF, 13 ms before the first
    // UPDATE.
    write16(reg::TCA0_PER, PWM_TOP);
    write16(reg::TCA0_CMP0BUF, 0);
    // The light's off, so PB0 stays PORT's until `set_output` asks for a
    // level.
    write(reg::TCA0_CTRLB, CTRLB_OFF);
    write(reg::TCA0_CTRLA, regs::TCA_CLKSEL_DIV1 | regs::TCA_ENABLE);

    // ADC0 in single-conversion mode, four conversions accumulated. The cell
    // and the temperature sensor both read against the 1.1 V reference with
    // the same timing, so only MUXPOS changes between them (`adc_channel`).
    // Turning it and the reference on waits for that too.
    //
    // The ATDF says CALIB resets to 50% duty, the datasheet says 25%, and 25%
    // is what it wants at ADC clocks of 1.5 MHz and under (section 30.5.17),
    // so say so.
    write(reg::ADC0_CALIB, regs::ADC_DUTYCYC_DUTY25);
    write(reg::ADC0_CTRLB, regs::ADC_SAMPNUM_ACC4);
    // 10 MHz / 16 is 625 kHz, inside the 200 kHz to 1.5 MHz the datasheet
    // wants for 10 bits (table 36-26). REFSEL stays INTREF, and SAMPCAP is
    // the smaller sample capacitor, for references of 1 V and up.
    write(
        reg::ADC0_CTRLC,
        regs::ADC_SAMPCAP | regs::ADC_REFSEL_INTREF | regs::ADC_PRESC_DIV16,
    );
    // 32 ADC clocks, 51 µs, over the 32 µs the temperature sensor needs
    // (section 30.3.2.6). This leaves SAMPDLY and ASDV at 0, which SAMPLEN
    // needs (errata DS80000886C 2.4.1).
    write(reg::ADC0_CTRLD, regs::ADC_INITDLY_DLY32);
    // 22 ADC clocks of sampling, 35 µs. The temperature sensor needs 32 µs,
    // and that sets this. PA1 is the other limit: the divider is 25 to 77 kΩ
    // depending on which story about its resistors is true (docs/hardware.md),
    // plus the ADC's own 14 kΩ, into 5 pF (table 36-25). At worst that's a
    // time constant of 0.46 µs, and settling to half a bit of 10 takes under
    // eight of those, 3.5 µs. The default two clocks would be 3.2 µs, so this
    // has ten times what it needs, where the default wouldn't.
    write(reg::ADC0_SAMPCTRL, regs::ADC_SAMPLEN_32US);

    // The RTC's periodic interrupt timer, off the internal 32.768 kHz
    // oscillator (RTC.CLKSEL's reset value) divided by 512: 64 Hz, with its
    // interrupt on, in the datasheet's order (section 23.5.1). RTC.CTRLA
    // stays at its reset value: any write to it stretches the PIT's period,
    // and clearing RTCEN stops the PIT (errata DS80000886C 2.6.1 and 2.6.2).
    write(reg::RTC_PITINTCTRL, regs::RTC_PI);
    while read(reg::RTC_PITSTATUS) & regs::RTC_CTRLBUSY != 0 {}
    write(reg::RTC_PITCTRLA, regs::RTC_PERIOD_CYC512 | regs::RTC_PITEN);
    // The first sleep can be power-down, and the datasheet doesn't say this
    // write finishes syncing with the peripheral clock stopped. If it didn't,
    // there'd be no ticks, so see it through.
    while read(reg::RTC_PITSTATUS) & regs::RTC_CTRLBUSY != 0 {}
}

/// The bit in GPIOR0 that [`rtc_pit`] sets each tick. GPIORs sit in I/O space,
/// which keeps the flag out of RAM and makes it one cycle to reach (AN2515C
/// section 4.5).
const TICK: u8 = 1 << 0;

/// The whole steps of PWM duty for [`tca0_ovf`], 0 to 255. Like the tick, the
/// delta-sigma's state lives in GPIORs: they're zero from reset, and this way
/// the handler reads them in one cycle each, with nothing to save.
const DSM_DUTY: usize = reg::GPIO_GPIOR1;
/// The seven bits of `Output::dsm` below the duty.
const DSM_FRACTION: usize = reg::GPIO_GPIOR2;
/// What's piled up of the fraction so far. Only the handler changes it while
/// the light's on.
const DSM_SUM: usize = reg::GPIO_GPIOR3;
/// The bits of `Output::dsm` that go in [`DSM_FRACTION`].
const FRACTION_MASK: u8 = (1 << DSM_BITS) - 1;

/// Sets up the sleep that [`wait_for_tick`] goes on to do. `rest` is
/// `watchdog::Feeder::power_down`.
///
/// Power-down stops TCA0 wherever it is, so it only happens with PB2 low. If
/// the torch ever says it's resting with the boost converter on, this idles
/// instead: the light stays as it was, and the loop keeps running, so the
/// protection does too.
///
/// It also turns the ADC and its forced reference off. The datasheet gives no
/// current for either left on in power-down, and Anduril's
/// `sleep_until_eswitch_pressed` does the same. [`adc_channel`] turns them
/// back on.
///
/// Otherwise it's idle, which stops only the CPU and flash, so TCA0 keeps the
/// PWM going and the ADC keeps converting. Standby would stop TCA0, which has
/// no RUNSTDBY (section 11.3.2.1).
///
/// SEN has to stay set in both: with it clear, `sleep` does nothing.
fn prepare_sleep(rest: bool) {
    let mode = if rest && read(reg::VPORTB_OUT) & 1 << ENABLE == 0 {
        write(reg::ADC0_CTRLA, 0);
        write(reg::VREF_CTRLB, 0);
        regs::SLPCTRL_SMODE_PDOWN
    } else {
        regs::SLPCTRL_SMODE_IDLE
    };
    write(reg::SLPCTRL_CTRLA, mode | regs::SLPCTRL_SEN);
}

/// Sleeps until the next 64 Hz tick, in power-down if `rest` allows it. The
/// PIT carries on in there (section 23.8 and the errata's table 3-3), so the
/// loop runs every tick, on or off, and that's how it sees the switch.
///
/// The switch has no interrupt, so a press never wakes the chip: it waits for
/// the next tick, 15.6 ms at most, and the loop reads PA5 then. A press that
/// comes and goes between two ticks is missed. An interrupt on the pin
/// wouldn't change either, since the loop only looks once a tick: the chip
/// would wake and go straight back to sleep. It wouldn't be dependable from
/// power-down anyway. PA5 is a partially asynchronous pin, so with the clock
/// stopped a press that's over before the chip has woken can wake it and
/// raise no interrupt (DS40002204A table 16-3, and table 5-1's note 2).
/// Nobody's measured how short a press this switch can give.
///
/// The PIT interrupt sets [`TICK`]; this checks and clears it with interrupts
/// off, so a tick can't land between the check and the clear. If it isn't set
/// yet, `sei` and `sleep` go back to back (AN2515C section 4.5): the
/// instruction after `sei` always runs before a pending interrupt, so a tick
/// that arrived after the check wakes the `sleep` at once rather than being
/// slept through. The only other interrupt is TCA0's overflow, which wakes it
/// from idle every PWM cycle while the light's on, and it goes round again.
///
/// Always true. On the TS21 false means a deadline ran out first, and this
/// board has no deadline, so a dead 32 kHz oscillator stops the loop here.
pub fn wait_for_tick(rest: bool) -> bool {
    prepare_sleep(rest);
    loop {
        // SAFETY: `cli` only clears SREG.I. With no `nomem`, it's also a
        // compiler barrier, so the GPIOR0 read can't move above it.
        unsafe { asm!("cli", options(nostack)) };
        if read(reg::GPIO_GPIOR0) & TICK != 0 {
            write(reg::GPIO_GPIOR0, 0);
            // SAFETY: `sei` only sets SREG.I.
            unsafe { asm!("sei", options(nostack)) };
            return true;
        }
        // SAFETY: sets SREG.I and sleeps until an interrupt, whose handler
        // returns here. The handler saves what it touches.
        unsafe { asm!("sei", "sleep", options(nostack)) };
    }
}

/// RTC_PIT, vector 7: the 64 Hz tick. It clears the PIT's flag and sets
/// [`TICK`], and that's all, since the loop only reads or clears that with
/// interrupts off.
#[unsafe(no_mangle)]
pub extern "avr-interrupt" fn rtc_pit() {
    write(reg::RTC_PITINTFLAGS, regs::RTC_PI);
    write(reg::GPIO_GPIOR0, TICK);
}

/// Points the ADC at `muxpos`, turning it and the 1.1 V reference on if a rest
/// turned them off.
///
/// The reference is forced on, rather than left to start when each single
/// conversion asks for it. The datasheet doesn't say how long a request lasts,
/// and a reference caught starting up reads wrong. Nothing else uses VREF, so
/// DAC0REFSEL's bits can stay zero.
fn adc_channel(muxpos: u8) {
    write(reg::VREF_CTRLB, regs::VREF_ADC0REFEN);
    write(reg::VREF_CTRLA, regs::VREF_ADC0REFSEL_1V1);
    write(reg::ADC0_MUXPOS, muxpos);
    write(reg::ADC0_CTRLA, regs::ADC_ENABLE);
}

/// Starts a conversion, four accumulated, setting the ADC up for its channel
/// first if asked. `sensors::Schedule` only asks when the channel changes, or
/// after a rest. It takes 260 µs or so and carries on through idle. Going to
/// rest turns the ADC off even if one's running, and the datasheet doesn't say
/// what becomes of it, which is why the first reading after a rest gets thrown
/// away.
///
/// It never clears RESRDY. That works because [`adc_result`] runs every tick
/// and reading the result clears it, so a stale one is never read twice.
pub fn adc_start(convert: Convert) {
    if convert.setup {
        adc_channel(match convert.channel {
            Channel::Voltage => regs::ADC_MUXPOS_AIN1,
            Channel::Temperature => regs::ADC_MUXPOS_TEMPSENSE,
        });
    }
    write(reg::ADC0_COMMAND, regs::ADC_STCONV);
}

/// The last conversion's result, if it's finished and not been read yet, from
/// 0 to 4092. Reading the high byte is what clears RESRDY (errata
/// DS80000886C 2.4.5), so both bytes always get read, low first for TEMP.
pub fn adc_result() -> Option<u16> {
    if read(reg::ADC0_INTFLAGS) & regs::ADC_RESRDY == 0 {
        return None;
    }
    let lo = read(reg::ADC0_RES);
    let hi = read(reg::ADC0_RES + 1);
    Some(u16::from_le_bytes([lo, hi]))
}

/// The temperature sensor's factory calibration from the signature row.
pub fn temperature_calibration() -> Calibration {
    Calibration {
        offset: read(reg::SIGROW_TEMPSENSE1) as i8,
        gain: read(reg::SIGROW_TEMPSENSE0),
    }
}

/// Starts the watchdog in window mode, 32 WDT clocks closed and 32 open. Until
/// the first feed there's no closed window, only the 32-clock timeout, and the
/// first pass of the loop feeds it.
///
/// It waits for the write to sync, so that feed can't land while it's still
/// syncing. The WDT shares its oscillator with the PIT, so if that's dead
/// these waits never end, and neither would the loop: the light's off at this
/// point, so that's where it stays. It keeps counting in every sleep mode,
/// which is fine, since the PIT does too and the loop runs every tick, on or
/// off. Never set WDT.STATUS.LOCK, and never touch the WDTCFG fuse.
pub fn start_watchdog() {
    while read(reg::WDT_STATUS) & regs::WDT_SYNCBUSY != 0 {}
    protected_write(
        reg::WDT_CTRLA,
        regs::WDT_WINDOW_32CLK | regs::WDT_PERIOD_32CLK,
    );
    while read(reg::WDT_STATUS) & regs::WDT_SYNCBUSY != 0 {}
}

/// Tells the watchdog the loop is still running. `main` calls this at the end
/// of a pass, after the output's been written, when `watchdog::Feeder` says
/// so. Too early or too late and the chip resets.
pub fn feed_watchdog() {
    // SAFETY: `wdr` only resets the watchdog's counter. It touches no memory
    // and no flags. No `nomem`, so the compiler can't move it above the
    // output's volatile stores.
    unsafe { asm!("wdr", options(nostack, preserves_flags)) }
}

/// What caused this reset, from `RSTCTRL.RSTFR`, leaving it clear for the
/// next one. The flags are sticky: writing 1 clears any of them, and a
/// power-on reset clears them all but PORF (section 12.5.1). No other reset
/// does, so this has to run on every boot before anything could reset the
/// chip.
pub fn take_reset_flags() -> u8 {
    let flags = read(reg::RSTCTRL_RSTFR);
    write(reg::RSTCTRL_RSTFR, flags);
    flags
}

/// Has CRCSCAN check all of flash against the CRC `cargo xtask hex` puts in
/// its last two bytes, and says whether it matched. Run it before turning
/// anything on: the scan stalls the CPU until it's done, 8,192 words at
/// three clocks each, about 7 ms at the reset clock of 20 MHz / 6.
///
/// STATUS.OK reads 1 from reset, before any scan, and the scan only starts
/// three cycles after ENABLE while the CPU carries on (section 27.3.1). So
/// `nop`s go between the write and the first read, or a read could see the
/// reset value and pass a flash that's wrong. Three would do if the three
/// cycles count from the end of the `sts`; the datasheet doesn't say, so
/// there's a fourth. Then it waits for BUSY to clear, which it should
/// already have done, since the CPU was stalled.
///
/// SRC = FLASH is the only choice with BOOTEND and APPEND at 0, and PRIORITY
/// the only mode. Writing ENABLE starts a scan even when it's already 1.
pub fn flash_ok() -> bool {
    write(
        reg::CRCSCAN_CTRLB,
        regs::CRCSCAN_SRC_FLASH | regs::CRCSCAN_MODE_PRIORITY,
    );
    write(reg::CRCSCAN_CTRLA, regs::CRCSCAN_ENABLE);
    // SAFETY: four `nop`s, which touch nothing.
    unsafe {
        asm!(
            "nop",
            "nop",
            "nop",
            "nop",
            options(nostack, preserves_flags)
        )
    };
    while read(reg::CRCSCAN_STATUS) & regs::CRCSCAN_BUSY != 0 {}
    read(reg::CRCSCAN_STATUS) & regs::CRCSCAN_OK != 0
}

/// The fuse bytes `selftest::Fuses::ok` checks, read from data space. This
/// only ever reads them.
pub fn fuses() -> Fuses {
    Fuses {
        wdtcfg: read(reg::FUSE_WDTCFG),
        osccfg: read(reg::FUSE_OSCCFG),
        syscfg0: read(reg::FUSE_SYSCFG0),
        append: read(reg::FUSE_APPEND),
        bootend: read(reg::FUSE_BOOTEND),
    }
}

/// Lights one aux channel, dimly, or none. Safe to call every tick.
///
/// PC1 to PC3 have only on and off, so dim comes from the pin's pull-up: the
/// lit channel is an input with its pull-up on, and the trickle through the
/// pull-up is the low setting, like the TS21's button LED. The others are
/// outputs at the 0 in OUT, which holds them dark. That this is what Anduril's
/// low aux setting does on this board is likely, and nobody's measured it here.
pub fn set_aux(colour: Aux) {
    let lit = board::aux_pin(colour);
    write(reg::VPORTC_DIR, PORTC_OUTPUTS & !lit);
    for pin in [AUX_BLUE, AUX_GREEN, AUX_RED] {
        let ctrl = if lit & 1 << pin != 0 {
            regs::PORT_PULLUPEN
        } else {
            0
        };
        write(reg::PORTC_PIN0CTRL + usize::from(pin), ctrl);
    }
}

/// Whether the switch is down. It pulls PA5 low against the pin's pull-up.
/// The loop calls this once a tick, and it's all the firmware knows of the
/// switch: PA5 has no interrupt (see [`wait_for_tick`]).
///
/// IN stops following the pin in power-down, since the clock that syncs it
/// has stopped (DS40002204A section 16.3.5), and the datasheet doesn't say
/// how many clocks it takes to catch up after a wake. The loop gets here once
/// the PIT's handler has run and returned, dozens of clocks on, which ought to
/// be plenty. Nobody's measured it.
pub fn switch_pressed() -> bool {
    read(reg::VPORTA_IN) & (1 << SWITCH) == 0
}

/// Runs `f` with interrupts held off, and puts SREG back after, so it's safe
/// whether they were on or not.
fn without_interrupts(f: impl FnOnce()) {
    let sreg = read(reg::CPU_SREG);
    // SAFETY: `cli` only clears SREG.I. With no `nomem`, it's also a compiler
    // barrier, so nothing in `f` can move above it.
    unsafe { asm!("cli", options(nostack)) };
    f();
    write(reg::CPU_SREG, sreg);
}

/// TCA0_OVF, vector 8: the bottom of a PWM cycle, 19,600 times a second, and
/// the only thing that writes the compare while the light's on.
///
/// This is a first-order delta-sigma: the fraction piles up in [`DSM_SUM`],
/// and each time it passes a whole step of duty, this cycle runs one step
/// brighter and the step comes off the pile. Over 128 cycles the duty averages
/// out to `dsm / 128`. Both bytes are under 128, so the sum fits in a `u8`,
/// and a duty of 255 has no fraction (`ramp::DSM_TOP`), so it can't wrap.
///
/// It's about 50 cycles of the 510 in a PWM cycle, since 8-bit sums in I/O
/// space need nothing from RAM. CMP0BUF lands at the next BOTTOM, a whole cycle
/// on, so it doesn't matter how late this runs within that.
///
/// CMP0BUF shares TCA0's TEMP byte with every other 16-bit TCA0 register
/// (DS40002204A section 8.5.6), and a write that's half done when this fires
/// would go wrong. `set_output` is the only other thing that touches them, and
/// it does that with this interrupt off and interrupts held off.
#[unsafe(no_mangle)]
pub extern "avr-interrupt" fn tca0_ovf() {
    write(reg::TCA0_INTFLAGS, regs::TCA_OVF);
    let sum = read(DSM_SUM).wrapping_add(read(DSM_FRACTION));
    write(DSM_SUM, sum & FRACTION_MASK);
    let duty = read(DSM_DUTY).wrapping_add(sum >> DSM_BITS);
    write16(reg::TCA0_CMP0BUF, u16::from(duty));
}

/// Turning on sets the duty, then raises the enable. Turning off drops the
/// enable, then zeroes the duty. The duty goes through CMP0BUF, so it lands
/// at the bottom of the PWM cycle, up to 51 µs after the write. That's well
/// inside the 180 µs the boost converter takes to start switching.
///
/// A change of level with the light on only touches [`DSM_DUTY`] and
/// [`DSM_FRACTION`]; [`tca0_ovf`] picks them up at the next BOTTOM. Interrupts
/// are held off so it can't see one byte of the new level and one of the old.
///
/// PB0 only belongs to TCA0 while the light's on. The datasheet doesn't say
/// what level a waveform output holds in power-down, with TCA0 stopped, so
/// going off hands the pin back to PORT, whose OUT is 0. CMP0 gets a direct
/// write too, so power-down can follow on this very pass. The datasheet
/// doesn't say whether that clears the waveform output before BOTTOM, so PB0
/// could go high for up to one cycle at the next turn-on, 51 µs, inside the
/// 180 µs above. That's my reasoning, and not checked on a real light.
pub fn set_output(output: Output) {
    if !output.enable {
        write(reg::VPORTB_OUT, read(reg::VPORTB_OUT) & !(1 << ENABLE));
        without_interrupts(|| {
            write(reg::TCA0_INTCTRL, 0);
            write(reg::TCA0_CTRLB, CTRLB_OFF);
            write16(reg::TCA0_CMP0BUF, 0);
            write16(reg::TCA0_CMP0, 0);
            write(DSM_DUTY, 0);
            write(DSM_FRACTION, 0);
            write(DSM_SUM, 0);
        });
        return;
    }

    let dsm = output.dsm.min(DSM_TOP);
    let duty = (dsm >> DSM_BITS) as u8;
    let fraction = dsm as u8 & FRACTION_MASK;
    let was_off = read(reg::VPORTB_OUT) & 1 << ENABLE == 0;
    without_interrupts(|| {
        write(DSM_DUTY, duty);
        write(DSM_FRACTION, fraction);
        if was_off {
            write16(reg::TCA0_CMP0BUF, u16::from(duty));
            write(reg::TCA0_CTRLB, CTRLB_ON);
            write(reg::TCA0_INTFLAGS, regs::TCA_OVF);
            write(reg::TCA0_INTCTRL, regs::TCA_OVF);
        }
    });
    write(reg::VPORTB_OUT, read(reg::VPORTB_OUT) | 1 << ENABLE);
}
