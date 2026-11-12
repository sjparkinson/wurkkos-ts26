//! What the checks at power-on found.
//!
//! The firmware reads the fuses back before it turns anything on (AN2632
//! §11), and checks the fields it relies on, not whole bytes. The datasheet,
//! the errata and the ATDF disagree about the factory bytes, so a whole-byte
//! compare would fail on a chip that's fine. It never writes them.
//!
//! It also has the chip's CRCSCAN check all of flash against the CRC-16 that
//! `cargo xtask hex` stores in its last two bytes (AN2632 §9.1), which catches
//! a flash that a `cargo xtask flash` left half-written. The hardware does the
//! sums, so all that's here is what a failure means.
//!
//! And a watchdog reset means something hung ([`hung`]).
//!
//! A failure doesn't leave a dead torch, because a light that won't come on
//! tells you nothing on the bench. It quietly caps the light instead, at
//! `limits::FAULT_CEILING`, until the cell comes out.
//!
//! That's the whole of the safety net. The RAM and register tests, the clock
//! check and a proof that the watchdog bites are left out on purpose (see
//! `watchdog`).

use crate::regs;

/// The fuse bytes the firmware checks, as read from data space.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fuses {
    pub wdtcfg: u8,
    pub osccfg: u8,
    pub syscfg0: u8,
    pub append: u8,
    pub bootend: u8,
}

impl Fuses {
    /// Whether every field the firmware relies on is what it expects.
    ///
    /// - `WDTCFG` is 0. A non-zero PERIOD starts the watchdog at boot, with
    ///   LOCK set (DS §19.3.1), so the firmware can't set it up. The datasheet
    ///   doesn't say what a WINDOW alone does, so the whole byte has to be 0.
    /// - `OSCCFG.FREQSEL` is 20 MHz and `OSCLOCK` clear. Every timing assumes
    ///   a calibrated 20 MHz divided by two, and OSCLOCK stops the calibration
    ///   loading (errata 2.2.1).
    /// - `SYSCFG0.RSTPINCFG` is UPDI, so the light can still be reflashed.
    /// - `APPEND` and `BOOTEND` are 0. The vector table is linked at 0, and
    ///   only the whole-flash CRC scan expects the checksum where `xtask`
    ///   puts it.
    pub const fn ok(self) -> bool {
        self.wdtcfg == 0
            && self.osccfg & regs::FUSE_FREQSEL == regs::FUSE_FREQSEL_20MHZ
            && self.osccfg & regs::FUSE_OSCLOCK == 0
            && self.syscfg0 & regs::FUSE_RSTPINCFG == regs::FUSE_RSTPINCFG_UPDI
            && self.append == 0
            && self.bootend == 0
    }
}

/// Whether `RSTCTRL.RSTFR`, as this boot found it, says the watchdog reset
/// the chip. The firmware's only ever reset by the watchdog if the loop
/// stopped feeding it, so that's [`Faults::hang`].
pub const fn hung(flags: u8) -> bool {
    flags & regs::RSTCTRL_WDRF != 0
}

/// Which checks failed at power-on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Faults {
    /// Flash doesn't match the CRC `cargo xtask hex` put at its end, by
    /// CRCSCAN's sums.
    pub flash: bool,
    /// The fuses aren't what the firmware expects.
    pub fuses: bool,
    /// The watchdog reset the chip, so something hung.
    pub hang: bool,
}

impl Faults {
    /// Every check passed.
    pub const NONE: Self = Self {
        flash: false,
        fuses: false,
        hang: false,
    };

    /// Whether anything failed, which caps the light.
    pub const fn any(self) -> bool {
        self.flash || self.fuses || self.hang
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Bytes a factory ATtiny1616 should hold, near enough. The fields the
    /// check reads are right, and the rest is as the datasheet has it.
    const GOOD: Fuses = Fuses {
        wdtcfg: 0x00,
        osccfg: 0x02,
        syscfg0: 0xC4,
        append: 0x00,
        bootend: 0x00,
    };

    #[test]
    fn factory_fuses_pass() {
        assert!(GOOD.ok());
    }

    #[test]
    fn checks_fields_not_whole_bytes() {
        // OSCCFG's middle bits are reserved, and SYSCFG0's CRCSRC and EESAVE
        // are none of the firmware's business. The ATDF's 0x7E and 0xF6 are
        // its idea of the bytes, and should pass too.
        for (osccfg, syscfg0) in [(0x7E, 0xF6), (0x02, 0x05), (0x7E, 0xC4), (0x02, 0x04)] {
            let fuses = Fuses {
                osccfg,
                syscfg0,
                ..GOOD
            };
            assert!(fuses.ok(), "{osccfg:#04x} {syscfg0:#04x}");
        }
    }

    #[test]
    fn each_wrong_field_fails() {
        // (what, which byte, its value), the bytes in `Fuses`' order.
        let bad = [
            ("watchdog on from the fuse", 0, 0x0B),
            ("watchdog window only", 0, 0x10),
            ("16 MHz", 1, 0x01),
            ("FREQSEL reserved", 1, 0x03),
            ("FREQSEL 0", 1, 0x00),
            ("OSCLOCK", 1, 0x82),
            ("reset pin as GPIO", 2, 0xC0),
            ("reset pin as reset", 2, 0xC8),
            ("application section", 3, 0x20),
            ("boot section", 4, 0x02),
        ];
        for (what, byte, value) in bad {
            let mut fuses = GOOD;
            let field = match byte {
                0 => &mut fuses.wdtcfg,
                1 => &mut fuses.osccfg,
                2 => &mut fuses.syscfg0,
                3 => &mut fuses.append,
                _ => &mut fuses.bootend,
            };
            *field = value;
            assert!(!fuses.ok(), "{what}");
        }
        let erased = Fuses {
            wdtcfg: 0xFF,
            osccfg: 0xFF,
            syscfg0: 0xFF,
            append: 0xFF,
            bootend: 0xFF,
        };
        assert!(!erased.ok());
    }

    #[test]
    fn a_watchdog_reset_is_a_hang_and_nothing_else_is() {
        assert!(hung(regs::RSTCTRL_WDRF));
        assert!(hung(regs::RSTCTRL_WDRF | 0x01), "whatever else is set");
        assert!(!hung(0));
        assert!(!hung(!regs::RSTCTRL_WDRF));
    }

    #[test]
    fn only_a_fault_caps_the_light() {
        assert!(!Faults::NONE.any());
        let flash = Faults {
            flash: true,
            ..Faults::NONE
        };
        let fuses = Faults {
            fuses: true,
            ..Faults::NONE
        };
        let hang = Faults {
            hang: true,
            ..Faults::NONE
        };
        assert!(flash.any() && fuses.any() && hang.any());
    }
}
