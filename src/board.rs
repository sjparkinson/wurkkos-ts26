//! The TS26 driver's pin map, as bit numbers within each port. docs/hardware.md
//! has where each one comes from.
//!
//! These are board facts, not registers. `src/avr/hw.rs` drives the pins
//! through `src/regs.rs`, and xtask's simulator wires its fake chip up the
//! same way from here.
//!
//! | Pin     | Job                                          |
//! |---------|----------------------------------------------|
//! | PA0     | UPDI, left alone                             |
//! | PA1     | Battery divider (AIN1)                       |
//! | PA5     | Side switch, active low, pulled up           |
//! | PB0     | Main LED PWM, TCA0 WO0                       |
//! | PB2     | Boost converter (MP3432) enable, high is on  |
//! | PC1-PC3 | Aux LEDs, blue, green, red                   |

use crate::ui::Aux;

/// PA0.
pub const UPDI: u8 = 0;
/// PA1.
pub const BATTERY: u8 = 1;
/// PA5.
pub const SWITCH: u8 = 5;
/// PB0.
pub const PWM: u8 = 0;
/// PB2, the dangerous one, which is low whenever the light's off.
pub const ENABLE: u8 = 2;
/// PC1.
pub const AUX_BLUE: u8 = 1;
/// PC2.
pub const AUX_GREEN: u8 = 2;
/// PC3.
pub const AUX_RED: u8 = 3;
/// All three aux pins, as a PORTC mask.
pub const AUX_PINS: u8 = 1 << AUX_BLUE | 1 << AUX_GREEN | 1 << AUX_RED;

/// The PORTC pin that lights `colour`, as a mask, or 0 for none.
pub const fn aux_pin(colour: Aux) -> u8 {
    match colour {
        Aux::Off => 0,
        Aux::Blue => 1 << AUX_BLUE,
        Aux::Red => 1 << AUX_RED,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_colour_lights_one_aux_pin_or_none() {
        assert_eq!(aux_pin(Aux::Off), 0);
        for colour in [Aux::Blue, Aux::Red] {
            let pin = aux_pin(colour);
            assert_eq!(pin.count_ones(), 1, "{colour:?}");
            assert_eq!(pin & !AUX_PINS, 0, "{colour:?}");
        }
        assert_ne!(aux_pin(Aux::Blue), aux_pin(Aux::Red));
    }
}
