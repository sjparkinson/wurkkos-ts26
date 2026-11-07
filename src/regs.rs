//! ATtiny1616 register addresses and bitfield values, typed in by hand from
//! Microchip's ATtiny1616 ATDF in `vendor/`.
//!
//! Only plain constants live here, so the file compiles anywhere. The
//! firmware uses it from `src/hw.rs` and `src/rt.rs`, and xtask's tests pull
//! it in by path and check every constant against the ATDF
//! (`xtask/src/atdf.rs`). Add a check there for anything you add here; an
//! unchecked constant shows up as dead code in the xtask build.
//!
//! Field values are already shifted into place, so they can be OR'd
//! straight into the register.

/// Data-space addresses.
pub mod addr {
    /// CPU.CCP. Below 0x40, so its I/O address is the same number and
    /// `out` can reach it.
    pub const CPU_CCP: usize = 0x0034;
    /// CPU.SREG, also within `out` range.
    pub const CPU_SREG: usize = 0x003F;

    /// VPORTA.IN: PA5 is the switch.
    pub const VPORTA_IN: usize = 0x0002;
    /// VPORTA.INTFLAGS: writing 1 to a pin's bit clears its flag.
    pub const VPORTA_INTFLAGS: usize = 0x0003;
    /// VPORTB.DIR.
    pub const VPORTB_DIR: usize = 0x0004;
    /// VPORTB.OUT: PB2 is the boost converter's enable.
    pub const VPORTB_OUT: usize = 0x0005;
    /// VPORTC.DIR.
    pub const VPORTC_DIR: usize = 0x0008;

    /// GPIO.GPIOR0, one of four spare bytes in I/O space. src/hw.rs says
    /// what each one holds.
    pub const GPIO_GPIOR0: usize = 0x001C;
    /// GPIO.GPIOR1.
    pub const GPIO_GPIOR1: usize = 0x001D;
    /// GPIO.GPIOR2.
    pub const GPIO_GPIOR2: usize = 0x001E;
    /// GPIO.GPIOR3.
    pub const GPIO_GPIOR3: usize = 0x001F;

    /// RSTCTRL.RSTFR: what caused the last reset.
    pub const RSTCTRL_RSTFR: usize = 0x0040;

    /// SLPCTRL.CTRLA.
    pub const SLPCTRL_CTRLA: usize = 0x0050;

    /// CLKCTRL.MCLKCTRLB: the main clock prescaler.
    pub const CLKCTRL_MCLKCTRLB: usize = 0x0061;

    /// VREF.CTRLA: which voltage each reference runs at.
    pub const VREF_CTRLA: usize = 0x00A0;
    /// VREF.CTRLB.
    pub const VREF_CTRLB: usize = 0x00A1;

    /// WDT.CTRLA, behind configuration change protection.
    pub const WDT_CTRLA: usize = 0x0100;
    /// WDT.STATUS.
    pub const WDT_STATUS: usize = 0x0101;

    /// CRCSCAN.CTRLA.
    pub const CRCSCAN_CTRLA: usize = 0x0120;
    /// CRCSCAN.CTRLB.
    pub const CRCSCAN_CTRLB: usize = 0x0121;
    /// CRCSCAN.STATUS.
    pub const CRCSCAN_STATUS: usize = 0x0122;

    /// RTC.PITCTRLA.
    pub const RTC_PITCTRLA: usize = 0x0150;
    /// RTC.PITSTATUS.
    pub const RTC_PITSTATUS: usize = 0x0151;
    /// RTC.PITINTCTRL.
    pub const RTC_PITINTCTRL: usize = 0x0152;
    /// RTC.PITINTFLAGS.
    pub const RTC_PITINTFLAGS: usize = 0x0153;

    /// PORTA.PIN0CTRL. PINnCTRL is PIN0CTRL + n.
    pub const PORTA_PIN0CTRL: usize = 0x0410;
    /// PORTB.PIN0CTRL.
    pub const PORTB_PIN0CTRL: usize = 0x0430;
    /// PORTC.PIN0CTRL.
    pub const PORTC_PIN0CTRL: usize = 0x0450;

    /// ADC0.CTRLA.
    pub const ADC0_CTRLA: usize = 0x0600;
    /// ADC0.CTRLB.
    pub const ADC0_CTRLB: usize = 0x0601;
    /// ADC0.CTRLC.
    pub const ADC0_CTRLC: usize = 0x0602;
    /// ADC0.CTRLD.
    pub const ADC0_CTRLD: usize = 0x0603;
    /// ADC0.SAMPCTRL.
    pub const ADC0_SAMPCTRL: usize = 0x0605;
    /// ADC0.MUXPOS.
    pub const ADC0_MUXPOS: usize = 0x0606;
    /// ADC0.COMMAND.
    pub const ADC0_COMMAND: usize = 0x0608;
    /// ADC0.INTFLAGS.
    pub const ADC0_INTFLAGS: usize = 0x060B;
    /// ADC0.RES, 16-bit: read the low byte first.
    pub const ADC0_RES: usize = 0x0610;
    /// ADC0.CALIB.
    pub const ADC0_CALIB: usize = 0x0616;

    /// TCA0.CTRLA (single mode).
    pub const TCA0_CTRLA: usize = 0x0A00;
    /// TCA0.CTRLB (single mode).
    pub const TCA0_CTRLB: usize = 0x0A01;
    /// TCA0.INTCTRL (single mode).
    pub const TCA0_INTCTRL: usize = 0x0A0A;
    /// TCA0.INTFLAGS (single mode).
    pub const TCA0_INTFLAGS: usize = 0x0A0B;
    /// TCA0.PER (single mode), 16-bit: write through `write16`, low byte
    /// first.
    pub const TCA0_PER: usize = 0x0A26;
    /// TCA0.CMP0 (single mode), 16-bit.
    pub const TCA0_CMP0: usize = 0x0A28;
    /// TCA0.CMP0BUF (single mode), 16-bit.
    pub const TCA0_CMP0BUF: usize = 0x0A38;

    /// SIGROW.TEMPSENSE0: the temperature sensor's gain.
    pub const SIGROW_TEMPSENSE0: usize = 0x1120;
    /// SIGROW.TEMPSENSE1: the temperature sensor's offset.
    pub const SIGROW_TEMPSENSE1: usize = 0x1121;

    /// FUSE.WDTCFG. The fuses are only ever read here, as data space.
    pub const FUSE_WDTCFG: usize = 0x1280;
    /// FUSE.OSCCFG.
    pub const FUSE_OSCCFG: usize = 0x1282;
    /// FUSE.SYSCFG0.
    pub const FUSE_SYSCFG0: usize = 0x1285;
    /// FUSE.APPEND.
    pub const FUSE_APPEND: usize = 0x1287;
    /// FUSE.BOOTEND.
    pub const FUSE_BOOTEND: usize = 0x1288;
}

/// CPU.CCP: the key that unlocks I/O registers behind configuration change
/// protection.
pub const CPU_CCP_IOREG: u8 = 0xD8;

/// CLKCTRL.MCLKCTRLB.PEN, prescaler enable.
pub const CLKCTRL_PEN: u8 = 0x01;
/// CLKCTRL.MCLKCTRLB.PDIV = 2X.
pub const CLKCTRL_PDIV_2X: u8 = 0x00;

/// PORTx.PINnCTRL.PULLUPEN.
pub const PORT_PULLUPEN: u8 = 0x08;
/// PORTx.PINnCTRL.ISC = BOTHEDGES.
pub const PORT_ISC_BOTHEDGES: u8 = 0x01;

/// RSTCTRL.RSTFR.WDRF, the watchdog caused the last reset.
pub const RSTCTRL_WDRF: u8 = 0x08;

/// SLPCTRL.CTRLA.SEN. With it clear, `sleep` does nothing.
pub const SLPCTRL_SEN: u8 = 0x01;
/// SLPCTRL.CTRLA.SMODE = IDLE.
pub const SLPCTRL_SMODE_IDLE: u8 = 0x00;
/// SLPCTRL.CTRLA.SMODE = PDOWN.
pub const SLPCTRL_SMODE_PDOWN: u8 = 0x02 << 1;

/// VREF.CTRLA.ADC0REFSEL = 1V1.
pub const VREF_ADC0REFSEL_1V1: u8 = 0x01 << 4;
/// VREF.CTRLB.ADC0REFEN, which forces the reference on.
pub const VREF_ADC0REFEN: u8 = 0x02;

/// WDT.CTRLA.PERIOD = 32CLK.
pub const WDT_PERIOD_32CLK: u8 = 0x03;
/// WDT.CTRLA.WINDOW = 32CLK.
pub const WDT_WINDOW_32CLK: u8 = 0x03 << 4;
/// WDT.STATUS.SYNCBUSY.
pub const WDT_SYNCBUSY: u8 = 0x01;

/// CRCSCAN.CTRLA.ENABLE.
pub const CRCSCAN_ENABLE: u8 = 0x01;
/// CRCSCAN.CTRLB.SRC = FLASH.
pub const CRCSCAN_SRC_FLASH: u8 = 0x00;
/// CRCSCAN.CTRLB.MODE = PRIORITY.
pub const CRCSCAN_MODE_PRIORITY: u8 = 0x00;
/// CRCSCAN.STATUS.BUSY.
pub const CRCSCAN_BUSY: u8 = 0x01;
/// CRCSCAN.STATUS.OK.
pub const CRCSCAN_OK: u8 = 0x02;

/// FUSE.OSCCFG.FREQSEL.
pub const FUSE_FREQSEL: u8 = 0x03;
/// FUSE.OSCCFG.FREQSEL = 20MHZ.
pub const FUSE_FREQSEL_20MHZ: u8 = 0x02;
/// FUSE.OSCCFG.OSCLOCK.
pub const FUSE_OSCLOCK: u8 = 0x80;
/// FUSE.SYSCFG0.RSTPINCFG.
pub const FUSE_RSTPINCFG: u8 = 0x0C;
/// FUSE.SYSCFG0.RSTPINCFG = UPDI.
pub const FUSE_RSTPINCFG_UPDI: u8 = 0x01 << 2;

/// ADC0.CTRLA.ENABLE.
pub const ADC_ENABLE: u8 = 0x01;
/// ADC0.CTRLB.SAMPNUM = ACC4.
pub const ADC_SAMPNUM_ACC4: u8 = 0x02;
/// ADC0.CTRLC.SAMPCAP, the smaller sample capacitor, for references of 1 V
/// and up.
pub const ADC_SAMPCAP: u8 = 0x40;
/// ADC0.CTRLC.REFSEL = INTREF.
pub const ADC_REFSEL_INTREF: u8 = 0x00;
/// ADC0.CTRLC.PRESC = DIV16.
pub const ADC_PRESC_DIV16: u8 = 0x03;
/// ADC0.CTRLD.INITDLY = DLY32.
pub const ADC_INITDLY_DLY32: u8 = 0x02 << 5;
/// ADC0.SAMPCTRL.SAMPLEN.
pub const ADC_SAMPLEN: u8 = 0x1F;
/// ADC0.SAMPCTRL.SAMPLEN: 20 more ADC clocks of sampling. Hand-picked, since
/// it's a count rather than a value the ATDF names; src/hw.rs says why 20.
pub const ADC_SAMPLEN_32US: u8 = 20;
/// ADC0.MUXPOS.MUXPOS = AIN1, PA1.
pub const ADC_MUXPOS_AIN1: u8 = 0x01;
/// ADC0.MUXPOS.MUXPOS = TEMPSENSE.
pub const ADC_MUXPOS_TEMPSENSE: u8 = 0x1E;
/// ADC0.COMMAND.STCONV.
pub const ADC_STCONV: u8 = 0x01;
/// ADC0.INTFLAGS.RESRDY.
pub const ADC_RESRDY: u8 = 0x01;
/// ADC0.CALIB.DUTYCYC = DUTY25.
pub const ADC_DUTYCYC_DUTY25: u8 = 0x01;

const _: () = assert!(ADC_SAMPLEN_32US & !ADC_SAMPLEN == 0);

/// TCA0.CTRLA.ENABLE (single mode).
pub const TCA_ENABLE: u8 = 0x01;
/// TCA0.CTRLA.CLKSEL = DIV1 (single mode).
pub const TCA_CLKSEL_DIV1: u8 = 0x00;
/// TCA0.CTRLB.CMP0EN (single mode).
pub const TCA_CMP0EN: u8 = 0x10;
/// TCA0.CTRLB.CMP2EN (single mode). WO2's pad is PB2, the boost enable, so
/// src/hw.rs must never set it.
pub const TCA_CMP2EN: u8 = 0x40;
/// TCA0.CTRLB.WGMODE = DSBOTTOM (single mode).
pub const TCA_WGMODE_DSBOTTOM: u8 = 0x07;
/// TCA0.INTCTRL.OVF and TCA0.INTFLAGS.OVF (single mode).
pub const TCA_OVF: u8 = 0x01;

/// RTC.PITCTRLA.PITEN.
pub const RTC_PITEN: u8 = 0x01;
/// RTC.PITCTRLA.PERIOD = CYC512.
pub const RTC_PERIOD_CYC512: u8 = 0x08 << 3;
/// RTC.PITSTATUS.CTRLBUSY.
pub const RTC_CTRLBUSY: u8 = 0x01;
/// RTC.PITINTCTRL.PI and RTC.PITINTFLAGS.PI.
pub const RTC_PI: u8 = 0x01;

/// Entries in the vector table: reset plus 30 interrupts.
pub const VECTORS: usize = 31;
/// The vector table slot of PORTA's pin interrupt.
pub const VECTOR_PORTA_PORT: usize = 3;
/// The slot of the RTC's periodic interrupt.
pub const VECTOR_RTC_PIT: usize = 7;
/// The slot of TCA0's overflow interrupt.
pub const VECTOR_TCA0_OVF: usize = 8;
