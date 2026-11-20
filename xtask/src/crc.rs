//! The checksum CRCSCAN checks flash against at power-on: CRC-16/CCITT-FALSE
//! (DS40002204A §27.2, and AN2521A §5), in the last two bytes of flash, high
//! byte first (§27.3.2.1, table 27-1). Unconfirmed on a real chip.

use crate::stamp::STAMP_LEN;
use crate::updi::map::FLASH_SIZE;

pub fn crc16(bytes: impl IntoIterator<Item = u8>) -> u16 {
    bytes.into_iter().fold(0xFFFF, |crc, byte| {
        (0..8).fold(crc ^ u16::from(byte) << 8, |crc, _| {
            if crc & 0x8000 != 0 {
                crc << 1 ^ 0x1021
            } else {
                crc << 1
            }
        })
    })
}

pub const CHECKSUM_AT: usize = FLASH_SIZE - 2;

pub const STAMP_AT: usize = CHECKSUM_AT - STAMP_LEN;

/// Padded with 0xFF, which is what erased flash reads as.
pub fn sign(image: &[u8], stamp: &[u8; STAMP_LEN]) -> Result<Vec<u8>, String> {
    if image.len() > STAMP_AT {
        return Err(format!(
            "the image is {} bytes, which leaves no room for the stamp and CRC at {STAMP_AT:#06x}",
            image.len()
        ));
    }
    let mut flash = image.to_vec();
    flash.resize(STAMP_AT, 0xFF);
    flash.extend(stamp);
    let checksum = crc16(flash.iter().copied());
    flash.extend(checksum.to_be_bytes());
    Ok(flash)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stamp;

    #[test]
    fn the_checksum_sits_at_the_end_of_16_kb() {
        assert_eq!(CHECKSUM_AT, 0x3FFE);
    }

    #[test]
    fn matches_the_crc_16_ccitt_false_check_value() {
        assert_eq!(crc16(*b"123456789"), 0x29B1);
        assert_eq!(crc16([]), 0xFFFF, "the seed");
    }

    #[test]
    fn is_the_unaugmented_crc_not_the_augmented_one() {
        // Not the augmented CCITT's 0x9479 (DS40002204A figure 27-1).
        assert_eq!(crc16(*b"A"), 0xB915);
    }

    #[test]
    fn feeds_bytes_most_significant_bit_first() {
        // Worked by hand.
        assert_eq!(crc16([0x80]), 0x7078);
        assert_ne!(crc16([0x80]), crc16([0x01]));
    }

    #[test]
    fn a_signed_image_fills_flash_and_ends_in_its_checksum() {
        let image = [0x0C, 0x94, 0x3E, 0x00];
        let flash = sign(&image, &stamp::TEST).unwrap();
        assert_eq!(flash.len(), FLASH_SIZE);
        assert_eq!(flash[..4], image);
        assert!(flash[4..STAMP_AT].iter().all(|&b| b == 0xFF));
        let checksum = crc16(flash[..CHECKSUM_AT].iter().copied());
        assert_eq!(
            flash[CHECKSUM_AT..],
            checksum.to_be_bytes(),
            "high byte first"
        );
    }

    #[test]
    fn the_whole_of_a_signed_flash_checks_to_zero() {
        for image in [&[][..], &[0x12, 0x34], &[0xAA; 4000]] {
            let flash = sign(image, &stamp::TEST).unwrap();
            assert_eq!(crc16(flash.iter().copied()), 0);
            for at in [
                0,
                1000,
                STAMP_AT,
                CHECKSUM_AT - 1,
                CHECKSUM_AT,
                FLASH_SIZE - 1,
            ] {
                let mut bad = flash.clone();
                bad[at] ^= 0x10;
                assert_ne!(crc16(bad.iter().copied()), 0, "{at}");
            }
        }
    }

    #[test]
    fn the_stamp_sits_just_before_the_checksum() {
        assert_eq!(STAMP_AT, CHECKSUM_AT - STAMP_LEN);
        let flash = sign(&[0x12, 0x34], &stamp::TEST).unwrap();
        assert_eq!(flash[STAMP_AT..CHECKSUM_AT], stamp::TEST);
        assert_eq!(stamp::read(&flash).as_deref(), Some("ts26 test"));
    }

    #[test]
    fn refuses_an_image_that_reaches_the_stamp() {
        assert!(sign(&vec![0; STAMP_AT], &stamp::TEST).is_ok());
        for len in [STAMP_AT + 1, CHECKSUM_AT, CHECKSUM_AT + 1] {
            let err = sign(&vec![0; len], &stamp::TEST).unwrap_err();
            assert!(err.contains("no room for the stamp and CRC"), "{err}");
        }
    }
}
