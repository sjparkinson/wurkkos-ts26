//! The pin map, as bit numbers within each port.

use crate::ui::Aux;

/// PA0, never written.
pub const UPDI: u8 = 0;
/// PA1, the cell's divider, AIN1. Never a pull-up.
pub const BATTERY: u8 = 1;
/// PA5, active low against its pull-up.
pub const SWITCH: u8 = 5;
/// PB0, TCA0's WO0.
pub const PWM: u8 = 0;
/// PB2, the MP3432's enable, high on. Low whenever the light's off, and never
/// a pull-up.
pub const ENABLE: u8 = 2;
/// PC1.
pub const AUX_BLUE: u8 = 1;
/// PC2.
pub const AUX_GREEN: u8 = 2;
/// PC3.
pub const AUX_RED: u8 = 3;
pub const AUX_PINS: u8 = 1 << AUX_BLUE | 1 << AUX_GREEN | 1 << AUX_RED;

pub const AUX: [(u8, u8); 3] = [
    (AUX_BLUE, 1 << AUX_BLUE),
    (AUX_GREEN, 1 << AUX_GREEN),
    (AUX_RED, 1 << AUX_RED),
];

pub const fn aux_pin(colour: Aux) -> u8 {
    match colour {
        Aux::Off => 0,
        Aux::Blue => 1 << AUX_BLUE,
        Aux::Red => 1 << AUX_RED,
        Aux::Yellow => 1 << AUX_RED | 1 << AUX_GREEN,
        Aux::White => AUX_PINS,
    }
}

/// The rest of [`aux_pin`]'s glow dimly.
pub const fn aux_bright(colour: Aux) -> u8 {
    match colour {
        Aux::Red => 1 << AUX_RED,
        Aux::Off | Aux::Blue | Aux::Yellow | Aux::White => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_colour_lights_the_pins_it_mixes_and_off_none() {
        assert_eq!(aux_pin(Aux::Off), 0);
        assert_eq!(aux_pin(Aux::Blue), 1 << AUX_BLUE);
        assert_eq!(aux_pin(Aux::Red), 1 << AUX_RED);
        assert_eq!(aux_pin(Aux::Yellow), 1 << AUX_RED | 1 << AUX_GREEN);
        assert_eq!(aux_pin(Aux::White), AUX_PINS);
        let colours = [Aux::Off, Aux::Blue, Aux::Red, Aux::Yellow, Aux::White];
        for (i, &a) in colours.iter().enumerate() {
            for &b in &colours[i + 1..] {
                assert_ne!(aux_pin(a), aux_pin(b), "{a:?} and {b:?}");
            }
        }
    }

    #[test]
    fn only_lockout_red_is_bright_and_bright_pins_are_lit_ones() {
        assert_eq!(aux_bright(Aux::Red), aux_pin(Aux::Red));
        for colour in [Aux::Off, Aux::Blue, Aux::Yellow, Aux::White] {
            assert_eq!(aux_bright(colour), 0, "{colour:?}");
        }
    }

    #[test]
    fn aux_table_matches_aux_pin() {
        let [blue, green, red] = AUX;
        assert_eq!(blue, (AUX_BLUE, aux_pin(Aux::Blue)));
        assert_eq!(red, (AUX_RED, aux_pin(Aux::Red)));
        assert_eq!(green, (AUX_GREEN, 1 << AUX_GREEN));
        let masks = AUX.iter().fold(0, |all, &(_, mask)| all | mask);
        assert_eq!(masks, AUX_PINS);
        for (pin, mask) in AUX {
            assert_eq!(mask, 1 << pin);
        }
    }
}
