//! What the torch sees, for the simulators, and the library's torch as they
//! drive it, so the library-side runs (`simulate.rs`) and the firmware on the
//! simulated chip (`sim.rs`) can be compared tick by tick.

use ts26::button::Event;
use ts26::regs::addr as reg;
use ts26::selftest::{Faults, Fuses};
use ts26::sensors::Channel;
use ts26::testing::{Adc, temperature_reading, voltage_reading};
use ts26::torch::Torch;

use crate::updi::map;

/// The temperature sensor calibration the simulated chip has in its signature
/// row, the same as the library tests'.
pub use ts26::testing::CALIBRATION;

/// The fuse bytes the simulated chip has, FUSE.WDTCFG to FUSE.BOOTEND.
pub const FUSE_BYTES: usize = 9;

/// Each fuse byte's name, by its offset from FUSE.WDTCFG. Offset 3 is reserved.
pub const FUSE_NAMES: [&str; FUSE_BYTES] = [
    "WDTCFG", "BODCFG", "OSCCFG", "", "TCD0CFG", "SYSCFG0", "SYSCFG1", "APPEND", "BOOTEND",
];

/// The fuses the firmware expects, as the datasheet has a factory chip: no
/// watchdog from the fuse, BOD off, 20 MHz, UPDI on the reset pin, no CRC at
/// start-up and no boot or application section. The datasheet, the errata and
/// the ATDF disagree on OSCCFG and SYSCFG0's spare bits, which is why the
/// firmware only checks fields.
pub const FACTORY_FUSES: [u8; FUSE_BYTES] = [0x00, 0x00, 0x02, 0xFF, 0x00, 0xC4, 0x07, 0x00, 0x00];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct World {
    /// The cell, in centivolts, as `battery::centivolts` would report it.
    pub cell: u16,
    pub kelvin: u16,
    /// The fuse bytes.
    pub fuses: [u8; FUSE_BYTES],
    /// Flash has a byte that doesn't match its CRC, as a half-finished flash
    /// would leave it. The firmware checks once, so only the first tick's counts.
    pub corrupt_flash: bool,
}

impl World {
    /// A cell at 3.8 V, a room at 25 °C and factory fuses.
    pub const NORMAL: Self = Self {
        cell: 380,
        kelvin: 298,
        fuses: FACTORY_FUSES,
        corrupt_flash: false,
    };

    /// The raw reading a conversion on `channel` gives.
    pub fn reading(&self, channel: Channel) -> u16 {
        match channel {
            Channel::Voltage => voltage_reading(self.cell),
            Channel::Temperature => temperature_reading(self.kelvin),
        }
    }

    /// The fuse at a data-space address, if it's one of `fuses` and not the
    /// reserved byte, which the datasheet doesn't say how reads.
    pub fn fuse(&self, addr: usize) -> Option<u8> {
        let offset = addr.checked_sub(usize::from(map::FUSES))?;
        if FUSE_NAMES.get(offset)?.is_empty() {
            return None;
        }
        self.fuses.get(offset).copied()
    }

    /// A world whose fuses fail the firmware's read-back, which is what a
    /// script's `fault` means: OSCCFG asks for 16 MHz.
    pub fn with_bad_fuse(mut self) -> Self {
        self.fuses[2] = 0x01;
        self
    }

    /// What the firmware's power-on checks find in this world.
    pub fn faults(&self) -> Faults {
        let at = |addr| self.fuse(addr).unwrap_or(0xFF);
        let fuses = Fuses {
            wdtcfg: at(reg::FUSE_WDTCFG),
            osccfg: at(reg::FUSE_OSCCFG),
            syscfg0: at(reg::FUSE_SYSCFG0),
            append: at(reg::FUSE_APPEND),
            bootend: at(reg::FUSE_BOOTEND),
        };
        Faults {
            fuses: !fuses.ok(),
            flash: self.corrupt_flash,
            ..Faults::NONE
        }
    }
}

/// The library's torch with an ADC that answers from a [`World`], the way the
/// firmware's loop drives it: the result of last tick's conversion in, this
/// tick's conversion out. A conversion measures the world as it was when it
/// started, as on the simulated chip.
#[derive(Clone, Debug)]
pub struct Rig {
    pub torch: Torch,
    adc: Adc,
}

impl Default for Rig {
    fn default() -> Self {
        Self::new(&World::NORMAL)
    }
}

impl Rig {
    /// A torch that's just had its cell put in, in `world`, which decides what
    /// its power-on checks find.
    pub fn new(world: &World) -> Self {
        Self {
            torch: Torch::new(CALIBRATION, world.faults()),
            adc: Adc::new(),
        }
    }

    pub fn tick(&mut self, pressed: bool, world: &World) -> Option<Event> {
        let event = self.torch.tick(pressed, self.adc.result());
        self.adc
            .start_with(self.torch.convert(), |channel| world.reading(channel));
        event
    }

    pub fn level(&self) -> u8 {
        self.torch.level()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_rig_starts_off() {
        assert_eq!(Rig::default().level(), 0);
    }

    #[test]
    fn factory_fuses_pass_and_a_bad_one_is_a_fault() {
        assert_eq!(World::NORMAL.faults(), Faults::NONE);
        assert!(World::NORMAL.with_bad_fuse().faults().fuses);
    }

    #[test]
    fn the_rig_hands_results_back_a_tick_later() {
        let mut rig = Rig::default();
        for _ in 0..1000 {
            rig.tick(false, &World::NORMAL);
        }
        assert_eq!(rig.level(), 0);
    }

    #[test]
    fn fuses_read_by_address_and_the_reserved_byte_doesnt() {
        let at = usize::from(map::FUSES);
        assert_eq!(World::NORMAL.fuse(at + 2), Some(0x02), "OSCCFG");
        assert_eq!(World::NORMAL.fuse(at + 3), None, "reserved");
        assert_eq!(World::NORMAL.fuse(at + 9), None, "past the end");
    }

    #[test]
    fn a_cell_and_a_temperature_read_as_the_library_expects() {
        let voltage = World::NORMAL.reading(Channel::Voltage);
        assert!(ts26::battery::centivolts(voltage).abs_diff(380) <= 1);
        let temperature = World::NORMAL.reading(Channel::Temperature);
        assert_eq!(temperature, temperature_reading(298));
    }
}
