use crate::regs;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fuses {
    pub wdtcfg: u8,
    pub osccfg: u8,
    pub syscfg0: u8,
    pub append: u8,
    pub bootend: u8,
}

impl Fuses {
    /// - A non-zero `WDTCFG` starts the watchdog at boot with LOCK set (DS40002204A §19.3.1).
    /// - `OSCLOCK` stops the calibration loading (errata 2.2.1).
    /// - `APPEND` and `BOOTEND` at 0 leave the CRC scan's checksum where `xtask` puts it.
    pub const fn ok(self) -> bool {
        self.wdtcfg == 0
            && self.osccfg & regs::FUSE_FREQSEL == regs::FUSE_FREQSEL_20MHZ
            && self.osccfg & regs::FUSE_OSCLOCK == 0
            && self.syscfg0 & regs::FUSE_RSTPINCFG == regs::FUSE_RSTPINCFG_UPDI
            && self.append == 0
            && self.bootend == 0
    }
}

pub const fn hung(flags: u8) -> bool {
    flags & regs::RSTCTRL_WDRF != 0
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Faults {
    pub flash: bool,
    pub fuses: bool,
    pub hang: bool,
}

impl Faults {
    pub const NONE: Self = Self {
        flash: false,
        fuses: false,
        hang: false,
    };

    pub const fn any(self) -> bool {
        self.flash || self.fuses || self.hang
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        // 0x7E and 0xF6 are the ATDF's idea of the bytes.
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
    fn only_a_fault_counts() {
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
