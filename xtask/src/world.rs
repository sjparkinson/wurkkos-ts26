//! What the simulated chip sees. That's a script's scene, which
//! `ts26::testing`'s `Rig` sees too, plus the fuse bytes, which only the
//! firmware reads.

use ts26::regs::addr as reg;
use ts26::selftest::{Faults, Fuses};
use ts26::sensors::Channel;
use ts26::testing::Scene;

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

/// The fuses the firmware expects, as the datasheet has a factory chip. That's
/// no watchdog from the fuse, BOD off, 20 MHz, UPDI on the reset pin, no CRC
/// at start-up and no boot or application section. The datasheet, the errata
/// and the ATDF disagree on OSCCFG and SYSCFG0's spare bits, which is why the
/// firmware only checks fields.
pub const FACTORY_FUSES: [u8; FUSE_BYTES] = [0x00, 0x00, 0x02, 0xFF, 0x00, 0xC4, 0x07, 0x00, 0x00];

/// FUSE.OSCCFG's offset in [`FACTORY_FUSES`].
const OSCCFG: usize = reg::FUSE_OSCCFG - reg::FUSE_WDTCFG;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct World {
    /// The cell, the temperature, and whether flash matches its CRC, which
    /// the firmware checks once, so only the first tick's counts.
    pub scene: Scene,
    /// The fuse bytes.
    pub fuses: [u8; FUSE_BYTES],
}

impl World {
    /// A cell at 3.8 V, a room at 25 °C and factory fuses.
    pub const NORMAL: Self = Self {
        scene: Scene::NORMAL,
        fuses: FACTORY_FUSES,
    };

    /// The raw reading a conversion on `channel` gives.
    pub fn reading(&self, channel: Channel) -> u16 {
        self.scene.reading(channel)
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
    /// script's `fault` means. OSCCFG asks for 16 MHz.
    pub fn with_bad_fuse(mut self) -> Self {
        self.fuses[OSCCFG] = 0x01;
        self.scene.fault = true;
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
            flash: self.scene.corrupt_flash,
            ..Faults::NONE
        }
    }
}

impl From<Scene> for World {
    /// The world a script's scene describes, `fault` being a bad fuse.
    fn from(scene: Scene) -> Self {
        let world = Self {
            scene,
            ..Self::NORMAL
        };
        if scene.fault {
            world.with_bad_fuse()
        } else {
            world
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ts26::testing::temperature_reading;

    #[test]
    fn factory_fuses_pass_and_a_bad_one_is_a_fault() {
        assert_eq!(World::NORMAL.faults(), Faults::NONE);
        assert!(World::NORMAL.with_bad_fuse().faults().fuses);
    }

    #[test]
    fn a_scripts_scene_is_the_same_world_and_finds_the_same_faults() {
        assert_eq!(World::from(Scene::NORMAL), World::NORMAL);
        let scene = Scene {
            cell: 295,
            kelvin: 353,
            fault: true,
            corrupt_flash: true,
        };
        let world = World::from(scene);
        assert_eq!(world.scene, scene);
        assert_eq!(world.faults(), scene.faults());
    }

    #[test]
    fn fuses_read_by_address_and_the_reserved_byte_doesnt() {
        assert_eq!(World::NORMAL.fuse(reg::FUSE_OSCCFG), Some(0x02), "OSCCFG");
        assert_eq!(World::NORMAL.fuse(reg::FUSE_OSCCFG + 1), None, "reserved");
        let end = usize::from(map::FUSES) + FUSE_BYTES;
        assert_eq!(World::NORMAL.fuse(end), None, "past the end");
    }

    #[test]
    fn a_cell_and_a_temperature_read_as_the_library_expects() {
        let voltage = World::NORMAL.reading(Channel::Voltage);
        assert!(ts26::battery::centivolts(voltage).abs_diff(380) <= 1);
        let temperature = World::NORMAL.reading(Channel::Temperature);
        assert_eq!(temperature, temperature_reading(Scene::NORMAL.kelvin));
    }
}
