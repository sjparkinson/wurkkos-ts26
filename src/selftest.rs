//! What the checks at power-on found.
//!
//! A failure doesn't leave a dead torch, because a light that won't come on
//! tells you nothing on the bench. It quietly caps the light instead, at
//! `limits::FAULT_CEILING`, until the cell comes out.
//!
//! That's the whole of the safety net. The RAM and register tests, the clock
//! check and the watchdog's proof that the TS21 runs are left out on purpose
//! (see `watchdog`).

/// Which checks failed at power-on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
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

    #[test]
    fn only_a_fault_caps_the_light() {
        assert!(!Faults::NONE.any());
        assert_eq!(Faults::default(), Faults::NONE);
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
