use ts26::regs::addr as reg;
use ts26::selftest::{Faults, Fuses};
use ts26::sensors::Channel;
use ts26::testing::Scene;

use crate::updi::map;

pub use ts26::testing::CALIBRATION;

pub const FUSE_BYTES: usize = 9;

/// Offset 3 is reserved.
pub const FUSE_NAMES: [&str; FUSE_BYTES] = [
    "WDTCFG", "BODCFG", "OSCCFG", "", "TCD0CFG", "SYSCFG0", "SYSCFG1", "APPEND", "BOOTEND",
];

/// The datasheet, the errata and the ATDF disagree on OSCCFG and SYSCFG0's
/// spare bits, so the firmware only checks fields.
pub const FACTORY_FUSES: [u8; FUSE_BYTES] = [0x00, 0x00, 0x02, 0xFF, 0x00, 0xC4, 0x07, 0x00, 0x00];

const OSCCFG: usize = reg::FUSE_OSCCFG - reg::FUSE_WDTCFG;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct World {
    /// The firmware checks flash's CRC once, so only the first tick's
    /// `corrupt_flash` counts.
    pub scene: Scene,
    pub fuses: [u8; FUSE_BYTES],
}

impl World {
    pub const NORMAL: Self = Self {
        scene: Scene::NORMAL,
        fuses: FACTORY_FUSES,
    };

    pub fn reading(&self, channel: Channel) -> u16 {
        self.scene.reading(channel)
    }

    /// The datasheet doesn't say how the reserved byte reads.
    pub fn fuse(&self, addr: usize) -> Option<u8> {
        let offset = addr.checked_sub(usize::from(map::FUSES))?;
        if FUSE_NAMES.get(offset)?.is_empty() {
            return None;
        }
        self.fuses.get(offset).copied()
    }

    pub const fn with_bad_fuse(mut self) -> Self {
        self.fuses[OSCCFG] = 0x01;
        self.scene.fault = true;
        self
    }

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
            adc_dead: true,
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
