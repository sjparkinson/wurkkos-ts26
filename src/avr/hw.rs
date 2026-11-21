#![allow(unsafe_code)]

use core::arch::asm;
use core::ptr::{
    read_volatile, with_exposed_provenance, with_exposed_provenance_mut, write_volatile,
};

use ts26::board::{
    self, AUX_PINS, BATTERY, DSM_DUTY, DSM_FRACTION, DSM_SUM, ENABLE, PWM, SWITCH, UPDI,
};
use ts26::ramp::{DSM_BITS, DSM_TOP, Output, PWM_TOP};
use ts26::selftest::Fuses;
use ts26::sensors::{Channel, Convert};
use ts26::temp::Calibration;
use ts26::ui::Aux;
use ts26::watchdog::TICK_DEADLINE_CYCLES;

use crate::regs::{self, addr as reg};

const PORTB_OUTPUTS: u8 = 1 << PWM | 1 << ENABLE;
const PORTB_PINS: u8 = 6;
const PORTC_OUTPUTS: u8 = AUX_PINS;
const PORTC_PINS: u8 = 4;

const CTRLB_OFF: u8 = regs::TCA_WGMODE_DSBOTTOM;
const CTRLB_ON: u8 = regs::TCA_CMP0EN | regs::TCA_WGMODE_DSBOTTOM;

const _: () = assert!(
    ENABLE == 2 && (CTRLB_OFF | CTRLB_ON) & regs::TCA_CMP2EN == 0,
    "WO2's pad is PB2, so CMP2EN would hand the boost enable to TCA0"
);

const _: () = {
    use ts26::watchdog::{CLOSED_CLOCKS, OPEN_CLOCKS, REST_CLOCKS, REST_TICKS};
    let (window, period) = (
        regs::WDT_WINDOW.trailing_zeros(),
        regs::RTC_PERIOD.trailing_zeros(),
    );
    assert!(8 << ((regs::WDT_WINDOW_32CLK >> window) - 1) == CLOSED_CLOCKS);
    assert!(8 << (regs::WDT_PERIOD_32CLK - 1) == OPEN_CLOCKS);
    assert!(8 << (regs::WDT_PERIOD_8KCLK - 1) == REST_CLOCKS);
    assert!(1 << ((regs::RTC_PERIOD_CYC32768 - regs::RTC_PERIOD_CYC512) >> period) == REST_TICKS);
};

#[expect(
    clippy::as_conversions,
    reason = "widening in a const, where From can't be called"
)]
const SWITCH_CTRL: usize = reg::PORTA_PIN0CTRL + SWITCH as usize;
const SENSE_OFF: u8 = regs::PORT_PULLUPEN | regs::PORT_ISC_INTDISABLE;
const SENSE_ON: u8 = regs::PORT_PULLUPEN | regs::PORT_ISC_BOTHEDGES;

fn write(addr: usize, value: u8) {
    // SAFETY: every caller passes a `reg` constant, a memory-mapped I/O register.
    unsafe { write_volatile(with_exposed_provenance_mut::<u8>(addr), value) }
}

fn read(addr: usize) -> u8 {
    // SAFETY: as for `write`.
    unsafe { read_volatile(with_exposed_provenance::<u8>(addr)) }
}

#[expect(
    clippy::arithmetic_side_effects,
    reason = "addr is the low byte of a 16-bit register in data space, far under usize::MAX"
)]
fn write16(addr: usize, value: u16) {
    let [lo, hi] = value.to_le_bytes();
    write(addr, lo);
    write(addr + 1, hi);
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::as_conversions,
    reason = "usize is 16 bits on AVR, so addr fits Z whole, and try_from would need a \
              fallback address that doesn't exist"
)]
fn protected_write(addr: usize, value: u8) {
    const _: () = assert!(
        reg::CPU_CCP < 0x40,
        "`out` only reaches the first 64 I/O addresses"
    );
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

pub fn hold_enable_low() {
    write(reg::VPORTB_OUT, 0);
    write(reg::VPORTB_DIR, PORTB_OUTPUTS);
}

pub fn init() {
    protected_write(
        reg::CLKCTRL_MCLKCTRLB,
        regs::CLKCTRL_PDIV_2X | regs::CLKCTRL_PEN,
    );

    for pin in 0..8 {
        let ctrl = match pin {
            UPDI => continue,
            BATTERY => regs::PORT_ISC_INPUT_DISABLE,
            SWITCH => SENSE_OFF,
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

    write(reg::VPORTB_DIR, PORTB_OUTPUTS);
    write(reg::VPORTC_DIR, PORTC_OUTPUTS);

    write16(reg::TCA0_PER, PWM_TOP);
    write16(reg::TCA0_CMP0BUF, 0);
    write(reg::TCA0_CTRLB, CTRLB_OFF);
    write(reg::TCA0_INTCTRL, 0);
    write(reg::TCA0_CTRLA, regs::TCA_CLKSEL_DIV1 | regs::TCA_ENABLE);

    write(reg::ADC0_CALIB, regs::ADC_DUTYCYC_DUTY25);
    write(reg::ADC0_CTRLB, regs::ADC_SAMPNUM_ACC4);
    write(
        reg::ADC0_CTRLC,
        regs::ADC_SAMPCAP | regs::ADC_REFSEL_INTREF | regs::ADC_PRESC_DIV16,
    );
    write(reg::ADC0_CTRLD, regs::ADC_INITDLY_DLY32);
    write(reg::ADC0_SAMPCTRL, regs::ADC_SAMPLEN_32US);

    write(reg::RTC_PITINTCTRL, regs::RTC_PI);
    set_pit(regs::RTC_PERIOD_CYC512);
}

#[inline(never)]
fn set_pit(period: u8) {
    while read(reg::RTC_PITSTATUS) & regs::RTC_CTRLBUSY != 0 {}
    write(reg::RTC_PITCTRLA, period | regs::RTC_PITEN);
    while read(reg::RTC_PITSTATUS) & regs::RTC_CTRLBUSY != 0 {}
}

const TICK: u8 = 1 << 0;

const FRACTION_MASK: u8 = (1 << DSM_BITS) - 1;

fn prepare_sleep(rest: bool) {
    let pb2 = read(reg::VPORTB_OUT) & 1 << ENABLE;
    let mode = if rest && pb2 == 0 && read(reg::TCA0_INTCTRL) == 0 {
        write(reg::ADC0_CTRLA, 0);
        write(reg::VREF_CTRLB, 0);
        regs::SLPCTRL_SMODE_PDOWN
    } else {
        regs::SLPCTRL_SMODE_IDLE
    };
    write(reg::SLPCTRL_CTRLA, mode | regs::SLPCTRL_SEN);
}

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
        if read(reg::TCA0_INTCTRL) != 0 {
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

pub fn rest() -> bool {
    prepare_sleep(true);
    // SAFETY: `cli` only clears SREG.I. No `nomem`, so the reads stay below it.
    unsafe { asm!("cli", options(nostack)) };
    if read(reg::GPIO_GPIOR0) & TICK == 0 && !switch_pressed() && read(SWITCH_CTRL) == SENSE_ON {
        // The `nop` lets a tick pending behind the switch's interrupt run before the `cli`.
        // SAFETY: sleeps until an interrupt, whose handler saves what it uses.
        unsafe { asm!("sei", "sleep", "nop", "cli", options(nostack)) };
    }
    let ticked = read(reg::GPIO_GPIOR0) & TICK != 0 && read(SWITCH_CTRL) == SENSE_ON;
    write(reg::GPIO_GPIOR0, 0);
    // SAFETY: `sei` only sets SREG.I.
    unsafe { asm!("sei", options(nostack)) };
    ticked
}

pub fn slow_down() {
    set_watchdog(regs::WDT_PERIOD_8KCLK);
    set_pit(regs::RTC_PERIOD_CYC32768);
    write(SWITCH_CTRL, SENSE_ON);
}

pub fn speed_up() {
    write(SWITCH_CTRL, SENSE_OFF);
    set_pit(regs::RTC_PERIOD_CYC512);
    write(reg::GPIO_GPIOR0, 0);
}

#[unsafe(no_mangle)]
pub extern "avr-interrupt" fn porta_port() {
    write(SWITCH_CTRL, SENSE_OFF);
    write(reg::PORTA_INTFLAGS, 1 << SWITCH);
}

#[unsafe(export_name = "__halt")]
pub fn halt() -> ! {
    // SAFETY: `cli` only clears SREG.I. No `nomem`, so nothing moves above it.
    unsafe { asm!("cli", options(nostack)) };
    set_output(Output::OFF);
    write(reg::RTC_PITINTCTRL, 0);
    write(SWITCH_CTRL, SENSE_OFF);
    prepare_sleep(true);
    loop {
        // SAFETY: `sleep` touches no memory and no flags.
        unsafe { asm!("sleep", options(nostack, preserves_flags)) };
    }
}

#[unsafe(no_mangle)]
pub extern "avr-interrupt" fn rtc_pit() {
    write(reg::RTC_PITINTFLAGS, regs::RTC_PI);
    write(reg::GPIO_GPIOR0, TICK);
}

fn adc_channel(muxpos: u8) {
    write(reg::VREF_CTRLB, regs::VREF_ADC0REFEN);
    write(reg::VREF_CTRLA, regs::VREF_ADC0REFSEL_1V1);
    write(reg::ADC0_MUXPOS, muxpos);
    write(reg::ADC0_CTRLA, regs::ADC_ENABLE);
}

pub fn adc_start(convert: Convert) {
    if convert.setup {
        adc_channel(match convert.channel {
            Channel::Voltage => regs::ADC_MUXPOS_AIN1,
            Channel::Ground => regs::ADC_MUXPOS_GND,
            Channel::Temperature => regs::ADC_MUXPOS_TEMPSENSE,
        });
    }
    write(reg::ADC0_COMMAND, regs::ADC_STCONV);
}

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

pub fn start_watchdog() {
    set_watchdog(regs::WDT_WINDOW_32CLK | regs::WDT_PERIOD_32CLK);
}

pub fn window_watchdog() {
    start_watchdog();
    feed_watchdog();
}

#[inline(never)]
fn set_watchdog(ctrla: u8) {
    while read(reg::WDT_STATUS) & regs::WDT_SYNCBUSY != 0 {}
    protected_write(reg::WDT_CTRLA, ctrla);
    while read(reg::WDT_STATUS) & regs::WDT_SYNCBUSY != 0 {}
}

pub fn feed_watchdog() {
    // SAFETY: `wdr` touches no memory and no flags. No `nomem`, so the
    // compiler can't move it above the output's volatile stores.
    unsafe { asm!("wdr", options(nostack, preserves_flags)) }
}

pub fn take_reset_flags() -> u8 {
    let flags = read(reg::RSTCTRL_RSTFR);
    write(reg::RSTCTRL_RSTFR, flags);
    flags
}

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

pub fn fuses() -> Fuses {
    Fuses {
        wdtcfg: read(reg::FUSE_WDTCFG),
        osccfg: read(reg::FUSE_OSCCFG),
        syscfg0: read(reg::FUSE_SYSCFG0),
        append: read(reg::FUSE_APPEND),
        bootend: read(reg::FUSE_BOOTEND),
    }
}

pub fn set_aux(colour: Aux) {
    let lit = board::aux_pin(colour);
    let dim = lit & !board::aux_bright(colour);
    write(reg::VPORTC_OUT, board::aux_bright(colour));
    write(reg::VPORTC_DIR, PORTC_OUTPUTS & !dim);
    for &(pin, mask) in &board::AUX {
        let ctrl = if dim & mask != 0 {
            regs::PORT_PULLUPEN
        } else {
            0
        };
        write(reg::PORTC_PIN0CTRL + usize::from(pin), ctrl);
    }
}

// Unmeasured: how soon IN follows the pin again after a wake from power-down.
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

#[unsafe(no_mangle)]
pub extern "avr-interrupt" fn tca0_ovf() {
    write(reg::TCA0_INTFLAGS, regs::TCA_OVF);
    let sum = read(DSM_SUM).wrapping_add(read(DSM_FRACTION));
    write(DSM_SUM, sum & FRACTION_MASK);
    let duty = read(DSM_DUTY).wrapping_add(sum >> DSM_BITS);
    write16(reg::TCA0_CMP0BUF, u16::from(duty));
}

pub fn set_output(output: Output) {
    if !output.enable {
        write(reg::VPORTB_OUT, read(reg::VPORTB_OUT) & !(1 << ENABLE));
    }
    if !output.enable && output.dsm == 0 {
        without_interrupts(|| {
            write(reg::TCA0_INTCTRL, 0);
            write(reg::TCA0_CTRLB, CTRLB_OFF);
            write16(reg::TCA0_CMP0BUF, 0);
            // Unconfirmed: whether this clears WO0 before the next BOTTOM.
            write16(reg::TCA0_CMP0, 0);
            write(DSM_DUTY, 0);
            write(DSM_FRACTION, 0);
            write(DSM_SUM, 0);
        });
        return;
    }

    let dsm = output.dsm.min(DSM_TOP);
    const _: () = assert!(
        DSM_TOP >> DSM_BITS <= 0xFF,
        "a duty past a byte would fall back to 0 with the enable high"
    );
    let duty = u8::try_from(dsm >> DSM_BITS).unwrap_or(0);
    let [low, _] = dsm.to_le_bytes();
    let fraction = low & FRACTION_MASK;
    let stopped = read(reg::TCA0_INTCTRL) == 0;
    without_interrupts(|| {
        write(DSM_DUTY, duty);
        write(DSM_FRACTION, fraction);
        if stopped {
            write16(reg::TCA0_CMP0BUF, u16::from(duty));
            write(reg::TCA0_CTRLB, CTRLB_ON);
            write(reg::TCA0_INTFLAGS, regs::TCA_OVF);
            write(reg::TCA0_INTCTRL, regs::TCA_OVF);
        }
    });
    if output.enable {
        write(reg::VPORTB_OUT, read(reg::VPORTB_OUT) | 1 << ENABLE);
    }
}
