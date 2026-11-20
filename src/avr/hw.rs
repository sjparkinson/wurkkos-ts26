//! The TS26 driver's pins and registers. Section and table numbers are
//! DS40002204A's, errata DS80000886C's.

#![allow(unsafe_code)]

use core::arch::asm;
use core::ptr::{
    read_volatile, with_exposed_provenance, with_exposed_provenance_mut, write_volatile,
};

use ts26::board::{self, AUX_PINS, BATTERY, ENABLE, PWM, SWITCH, UPDI};
use ts26::ramp::{DSM_BITS, DSM_TOP, Output, PWM_TOP};
use ts26::selftest::Fuses;
use ts26::sensors::{Channel, Convert};
use ts26::temp::Calibration;
use ts26::ui::Aux;
use ts26::watchdog::TICK_DEADLINE_CYCLES;

use crate::regs::{self, addr as reg};

const PORTB_OUTPUTS: u8 = 1 << PWM | 1 << ENABLE;
/// PB0 to PB5 on the 20-pin package.
const PORTB_PINS: u8 = 6;
const PORTC_OUTPUTS: u8 = AUX_PINS;
/// PC0 to PC3 on the 20-pin package.
const PORTC_PINS: u8 = 4;

const CTRLB_OFF: u8 = regs::TCA_WGMODE_DSBOTTOM;
const CTRLB_ON: u8 = regs::TCA_CMP0EN | regs::TCA_WGMODE_DSBOTTOM;

// WO2's pad is PB2, so CMP2EN would hand the boost enable to TCA0.
const _: () = assert!(ENABLE == 2 && (CTRLB_OFF | CTRLB_ON) & regs::TCA_CMP2EN == 0);

// WDT.CTRLA's fields count 8 << (n - 1) WDT clocks.
const _: () = {
    use ts26::watchdog::{CLOSED_CLOCKS, OPEN_CLOCKS};
    assert!(8 << ((regs::WDT_WINDOW_32CLK >> 4) - 1) == CLOSED_CLOCKS);
    assert!(8 << (regs::WDT_PERIOD_32CLK - 1) == OPEN_CLOCKS);
};

fn write(addr: usize, value: u8) {
    // SAFETY: every caller passes a `reg` constant, a memory-mapped I/O register.
    unsafe { write_volatile(with_exposed_provenance_mut::<u8>(addr), value) }
}

fn read(addr: usize) -> u8 {
    // SAFETY: as for `write`.
    unsafe { read_volatile(with_exposed_provenance::<u8>(addr)) }
}

/// Low byte first, then high, through the shared TEMP byte (section 8.5.6).
#[expect(
    clippy::arithmetic_side_effects,
    reason = "addr is the low byte of a 16-bit register in data space, far under usize::MAX"
)]
fn write16(addr: usize, value: u16) {
    let [lo, hi] = value.to_le_bytes();
    write(addr, lo);
    write(addr + 1, hi);
}

/// The write only lands within four instructions of the CCP key, so it's
/// assembly, with the store straight after the key.
#[expect(
    clippy::cast_possible_truncation,
    clippy::as_conversions,
    reason = "usize is 16 bits on AVR, so addr fits Z whole, and try_from would need a \
              fallback address that doesn't exist"
)]
fn protected_write(addr: usize, value: u8) {
    // `out` only reaches the first 64 I/O addresses.
    const _: () = assert!(reg::CPU_CCP < 0x40);
    // SAFETY: writes the IOREG key to CPU.CCP, then `value` to `addr`, no Rust
    // memory and no flags.
    unsafe {
        asm!(
            "out {ccp}, {key}",
            "st Z, {value}",
            ccp = const reg::CPU_CCP,
            key = in(reg) regs::CPU_CCP_IOREG,
            value = in(reg) value,
            in("Z") addr as u16,
            options(nostack, preserves_flags),
        );
    }
}

/// First in `main`, since PB2 floats from reset until it's an output, and
/// [`flash_ok`] stalls the CPU for about 7 ms before [`init`]. OUT before DIR,
/// since a start that wasn't a reset could find PB2 still set. Nothing else
/// such a start leaves stale can raise PB2, so there's no software reset at
/// boot (reasoning only, unconfirmed on a chip).
pub fn hold_enable_low() {
    write(reg::VPORTB_OUT, 0);
    write(reg::VPORTB_DIR, PORTB_OUTPUTS);
}

pub fn init() {
    // 10 MHz, specified down to 2.7 V of VCC. How the regulator's VCC behaves
    // as the cell sags is unmeasured. `watchdog::CLOCK_HZ` repeats it by hand.
    protected_write(
        reg::CLKCTRL_MCLKCTRLB,
        regs::CLKCTRL_PDIV_2X | regs::CLKCTRL_PEN,
    );

    // PA0 is UPDI, and nothing here writes it. The outputs, PB2 above all,
    // never get a pull-up, which could creep the boost converter on. PA1, the
    // battery divider, gets its input buffer off (section 30.3.1.1) and never
    // a pull-up, which would skew the divider. PA5 keeps its input buffer, so
    // VPORTA.IN still reads the switch (section 16.5.11).
    for pin in 0..8 {
        let ctrl = match pin {
            UPDI => continue,
            BATTERY => regs::PORT_ISC_INPUT_DISABLE,
            SWITCH => regs::PORT_PULLUPEN | regs::PORT_ISC_INTDISABLE,
            _ => regs::PORT_PULLUPEN,
        };
        write(reg::PORTA_PIN0CTRL + usize::from(pin), ctrl);
    }
    for pin in 0..PORTB_PINS {
        if PORTB_OUTPUTS & 1 << pin == 0 {
            write(reg::PORTB_PIN0CTRL + usize::from(pin), regs::PORT_PULLUPEN);
        }
    }
    for pin in 0..PORTC_PINS {
        if PORTC_OUTPUTS & 1 << pin == 0 {
            write(reg::PORTC_PIN0CTRL + usize::from(pin), regs::PORT_PULLUPEN);
        }
    }

    // Both OUTs are zero by now, so these come up low.
    write(reg::VPORTB_DIR, PORTB_OUTPUTS);
    write(reg::VPORTC_DIR, PORTC_OUTPUTS);

    // PER goes in with the timer stopped, or the first cycle would run at the
    // reset value of 0xFFFF, 13 ms.
    write16(reg::TCA0_PER, PWM_TOP);
    write16(reg::TCA0_CMP0BUF, 0);
    write(reg::TCA0_CTRLB, CTRLB_OFF);
    write(reg::TCA0_CTRLA, regs::TCA_CLKSEL_DIV1 | regs::TCA_ENABLE);

    // The ATDF says CALIB resets to 50% duty and the datasheet 25%, which it
    // wants at ADC clocks of 1.5 MHz and under (section 30.5.17).
    write(reg::ADC0_CALIB, regs::ADC_DUTYCYC_DUTY25);
    write(reg::ADC0_CTRLB, regs::ADC_SAMPNUM_ACC4);
    // 625 kHz, inside table 36-26's 200 kHz to 1.5 MHz for 10 bits. SAMPCAP
    // is for references of 1 V and up.
    write(
        reg::ADC0_CTRLC,
        regs::ADC_SAMPCAP | regs::ADC_REFSEL_INTREF | regs::ADC_PRESC_DIV16,
    );
    // 51 µs, over the temperature sensor's 32 µs (section 30.3.2.6). SAMPDLY
    // and ASDV stay 0, as SAMPLEN needs (errata 2.4.1).
    write(reg::ADC0_CTRLD, regs::ADC_INITDLY_DLY32);
    // 35 µs, set by the temperature sensor's 32 µs. PA1's divider settles in
    // under 4 µs at worst (table 36-25).
    write(reg::ADC0_SAMPCTRL, regs::ADC_SAMPLEN_32US);

    // 64 Hz, in section 23.5.1's order. RTC.CTRLA keeps its reset value, since
    // a write stretches the PIT's period and clearing RTCEN stops it (errata
    // 2.6.1 and 2.6.2).
    write(reg::RTC_PITINTCTRL, regs::RTC_PI);
    while read(reg::RTC_PITSTATUS) & regs::RTC_CTRLBUSY != 0 {}
    write(reg::RTC_PITCTRLA, regs::RTC_PERIOD_CYC512 | regs::RTC_PITEN);
    // The first sleep can be power-down, and the datasheet doesn't say this
    // sync finishes there.
    while read(reg::RTC_PITSTATUS) & regs::RTC_CTRLBUSY != 0 {}
}

const TICK: u8 = 1 << 0;

/// The delta-sigma's state lives in GPIORs, so [`tca0_ovf`] needs nothing from
/// RAM.
const DSM_DUTY: usize = reg::GPIO_GPIOR1;
const DSM_FRACTION: usize = reg::GPIO_GPIOR2;
const DSM_SUM: usize = reg::GPIO_GPIOR3;
const FRACTION_MASK: u8 = (1 << DSM_BITS) - 1;

/// Power-down stops TCA0, so with PB2 high this idles instead. The ADC and its
/// forced reference go off first, since the datasheet gives no current for
/// either left on in power-down. Standby would stop TCA0, which has no
/// RUNSTDBY (section 11.3.2.1). SEN has to stay set.
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

/// The PIT runs in power-down (section 23.8, errata table 3-3). PA5 has no
/// interrupt, since the loop only looks once a tick, and a partially
/// asynchronous pin can wake power-down without raising one (table 16-3).
///
/// `sei` and `sleep` stay back to back, since the instruction after `sei` runs
/// before a pending interrupt, so a late tick still wakes the `sleep`.
///
/// `false` means the light's on and no tick came in `TICK_DEADLINE_CYCLES` PWM
/// cycles, about 104 ms, since the tick and the watchdog share the 32 kHz
/// oscillator. With PB2 low there's no deadline on purpose, so a dead
/// oscillator leaves the chip asleep with the boost converter off. Unconfirmed
/// on a real chip.
pub fn wait_for_tick(rest: bool) -> bool {
    prepare_sleep(rest);
    let mut cycles = TICK_DEADLINE_CYCLES;
    let ticked = loop {
        // SAFETY: `cli` only clears SREG.I. No `nomem`, so the GPIOR0 read
        // stays below it.
        unsafe { asm!("cli", options(nostack)) };
        if read(reg::GPIO_GPIOR0) & TICK != 0 {
            write(reg::GPIO_GPIOR0, 0);
            break true;
        }
        if read(reg::VPORTB_OUT) & 1 << ENABLE != 0 {
            let Some(left) = cycles.checked_sub(1) else {
                break false;
            };
            cycles = left;
        }
        // SAFETY: sleeps until an interrupt, whose handler saves what it uses.
        unsafe { asm!("sei", "sleep", options(nostack)) };
    };
    // SAFETY: `sei` only sets SREG.I.
    unsafe { asm!("sei", options(nostack)) };
    ticked
}

/// Also the panic handler, and reached from an unhandled interrupt with no
/// `reti`, so LVL0EX stays set (section 13.5.2). This core doesn't clear
/// SREG.I on the way into a handler (section 8.7.3), hence the `cli`.
///
/// With no interrupt enabled only a reset wakes the chip (section 11.3.1).
/// Figure 13-1 suggests `cli` alone would do, but no sentence says so, hence
/// both, and the `sleep` loops. With the 32 kHz oscillator running, the unfed
/// watchdog then resets the chip, and with it stopped the chip stays here,
/// off. Neither is confirmed on a real chip.
pub fn halt() -> ! {
    // SAFETY: `cli` only clears SREG.I. No `nomem`, so nothing moves above it.
    unsafe { asm!("cli", options(nostack)) };
    set_output(Output::OFF);
    write(reg::RTC_PITINTCTRL, 0);
    prepare_sleep(true);
    loop {
        // SAFETY: `sleep` touches no memory and no flags.
        unsafe { asm!("sleep", options(nostack, preserves_flags)) };
    }
}

/// RTC_PIT, vector 7.
#[unsafe(no_mangle)]
pub extern "avr-interrupt" fn rtc_pit() {
    write(reg::RTC_PITINTFLAGS, regs::RTC_PI);
    write(reg::GPIO_GPIOR0, TICK);
}

/// The reference is forced on, since the datasheet doesn't say how long a
/// conversion's request lasts, and one caught starting up reads wrong.
fn adc_channel(muxpos: u8) {
    write(reg::VREF_CTRLB, regs::VREF_ADC0REFEN);
    write(reg::VREF_CTRLA, regs::VREF_ADC0REFSEL_1V1);
    write(reg::ADC0_MUXPOS, muxpos);
    write(reg::ADC0_CTRLA, regs::ADC_ENABLE);
}

/// Never clears RESRDY. [`adc_result`] runs every tick, and its read does.
/// What a rest does to a running conversion isn't in the datasheet, which is
/// why the first reading after one is thrown away.
pub fn adc_start(convert: Convert) {
    if convert.setup {
        adc_channel(match convert.channel {
            Channel::Voltage => regs::ADC_MUXPOS_AIN1,
            Channel::Temperature => regs::ADC_MUXPOS_TEMPSENSE,
        });
    }
    write(reg::ADC0_COMMAND, regs::ADC_STCONV);
}

/// From 0 to 4092. Reading the high byte clears RESRDY (errata 2.4.5), so both
/// are read, low first for TEMP.
pub fn adc_result() -> Option<u16> {
    if read(reg::ADC0_INTFLAGS) & regs::ADC_RESRDY == 0 {
        return None;
    }
    let lo = read(reg::ADC0_RES);
    let hi = read(reg::ADC0_RES + 1);
    Some(u16::from_le_bytes([lo, hi]))
}

pub fn temperature_calibration() -> Calibration {
    Calibration {
        offset: read(reg::SIGROW_TEMPSENSE1).cast_signed(),
        gain: read(reg::SIGROW_TEMPSENSE0),
    }
}

/// Waits for the sync, so the first feed can't land in it. With a dead 32 kHz
/// oscillator these never end, with the light off. Never set WDT.STATUS.LOCK,
/// and never touch the WDTCFG fuse.
pub fn start_watchdog() {
    while read(reg::WDT_STATUS) & regs::WDT_SYNCBUSY != 0 {}
    protected_write(
        reg::WDT_CTRLA,
        regs::WDT_WINDOW_32CLK | regs::WDT_PERIOD_32CLK,
    );
    while read(reg::WDT_STATUS) & regs::WDT_SYNCBUSY != 0 {}
}

pub fn feed_watchdog() {
    // SAFETY: `wdr` touches no memory and no flags. No `nomem`, so the
    // compiler can't move it above the output's volatile stores.
    unsafe { asm!("wdr", options(nostack, preserves_flags)) }
}

/// The flags are sticky, writing 1 clears them, and only a power-on reset
/// clears them otherwise (section 12.5.1), so this runs before anything could
/// reset the chip.
pub fn take_reset_flags() -> u8 {
    let flags = read(reg::RSTCTRL_RSTFR);
    write(reg::RSTCTRL_RSTFR, flags);
    flags
}

/// Checks flash against the CRC `cargo xtask hex` puts in its last two bytes,
/// stalling the CPU for about 7 ms. STATUS.OK reads 1 until the scan starts
/// three cycles after ENABLE (section 27.3.1), hence the `nop`s, plus one
/// since the datasheet doesn't say where the three count from.
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

/// Only ever reads them.
pub fn fuses() -> Fuses {
    Fuses {
        wdtcfg: read(reg::FUSE_WDTCFG),
        osccfg: read(reg::FUSE_OSCCFG),
        syscfg0: read(reg::FUSE_SYSCFG0),
        append: read(reg::FUSE_APPEND),
        bootend: read(reg::FUSE_BOOTEND),
    }
}

/// Dim is an input with its pull-up on, bright an output at 1, dark an output
/// at 0. That these match the stock firmware's low and high is likely, and
/// unmeasured here. OUT goes first, so a channel going dark is never left
/// driven high on its way there.
pub fn set_aux(colour: Aux) {
    let lit = board::aux_pin(colour);
    let dim = lit & !board::aux_bright(colour);
    write(reg::VPORTC_OUT, board::aux_bright(colour));
    write(reg::VPORTC_DIR, PORTC_OUTPUTS & !dim);
    // The masks come from `board::AUX`, so there's no shift to check at run time.
    for &(pin, mask) in &board::AUX {
        let ctrl = if dim & mask != 0 {
            regs::PORT_PULLUPEN
        } else {
            0
        };
        write(reg::PORTC_PIN0CTRL + usize::from(pin), ctrl);
    }
}

/// IN stops following the pin in power-down (section 16.3.5), and how long it
/// takes to catch up after a wake isn't in the datasheet and is unmeasured.
pub fn switch_pressed() -> bool {
    read(reg::VPORTA_IN) & (1 << SWITCH) == 0
}

fn without_interrupts(f: impl FnOnce()) {
    let sreg = read(reg::CPU_SREG);
    // SAFETY: `cli` only clears SREG.I. No `nomem`, so nothing in `f` moves
    // above it.
    unsafe { asm!("cli", options(nostack)) };
    f();
    write(reg::CPU_SREG, sreg);
}

/// TCA0_OVF, vector 8, a first-order delta-sigma. Both bytes are under 128,
/// and a duty of 255 has no fraction (`ramp::DSM_TOP`), so nothing wraps.
/// CMP0BUF shares TCA0's TEMP byte (section 8.5.6), so [`set_output`] writes
/// TCA0's 16-bit registers with this interrupt off and interrupts held off.
#[unsafe(no_mangle)]
pub extern "avr-interrupt" fn tca0_ovf() {
    write(reg::TCA0_INTFLAGS, regs::TCA_OVF);
    let sum = read(DSM_SUM).wrapping_add(read(DSM_FRACTION));
    write(DSM_SUM, sum & FRACTION_MASK);
    let duty = read(DSM_DUTY).wrapping_add(sum >> DSM_BITS);
    write16(reg::TCA0_CMP0BUF, u16::from(duty));
}

/// On sets the duty, then raises the enable. Off drops the enable, then zeroes
/// the duty. CMP0BUF lands up to 51 µs later, inside the 180 µs the boost
/// converter takes to start switching.
///
/// Off hands PB0 back to PORT, since what a stopped TCA0 leaves on WO0 isn't
/// in the datasheet, and writes CMP0 directly so power-down can follow on this
/// pass. Whether that clears WO0 before BOTTOM is unconfirmed, which could
/// give PB0 one high cycle at the next turn-on.
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
    // So a `PWM_TOP` or `DSM_BITS` that would make the fallback a duty of 0
    // with the enable high fails the build instead.
    const _: () = assert!(DSM_TOP >> DSM_BITS <= 0xFF);
    let duty = u8::try_from(dsm >> DSM_BITS).unwrap_or(0);
    let [low, _] = dsm.to_le_bytes();
    let fraction = low & FRACTION_MASK;
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
