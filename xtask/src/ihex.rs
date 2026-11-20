//! Intel HEX, the format avrdude and pymcuprog both read.

use std::fmt::Write;

/// No extended address records, so 64 KB at most.
pub fn encode(image: &[u8]) -> String {
    assert!(
        image.len() <= 0x1_0000,
        "image too big for 16-bit addresses"
    );

    let mut out = String::new();
    for (i, chunk) in image.chunks(16).enumerate() {
        record(&mut out, (i * 16) as u16, 0x00, chunk);
    }
    record(&mut out, 0, 0x01, &[]);
    out
}

pub fn decode(text: &str) -> Result<Vec<u8>, String> {
    let mut image = Vec::new();
    let mut base = 0usize;

    for (n, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let at = |msg: &str| format!("line {}: {msg}", n + 1);

        let hex = line
            .strip_prefix(':')
            .ok_or_else(|| at("doesn't start with ':'"))?;
        if hex.len() % 2 != 0 || hex.len() < 10 {
            return Err(at("malformed record"));
        }
        let bytes = (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok())
            .collect::<Option<Vec<u8>>>()
            .filter(|_| hex.bytes().all(|b| b.is_ascii_hexdigit()))
            .ok_or_else(|| at("not hex"))?;

        let len = usize::from(bytes[0]);
        if bytes.len() != len + 5 {
            return Err(at("length doesn't match"));
        }
        if sum(&bytes) != 0 {
            return Err(at("bad checksum"));
        }
        let address = usize::from(u16::from_be_bytes([bytes[1], bytes[2]]));
        let data = &bytes[4..4 + len];

        match bytes[3] {
            0x00 => {
                let start = base + address;
                if image.len() < start + len {
                    image.resize(start + len, 0xFF);
                }
                image[start..start + len].copy_from_slice(data);
            }
            0x01 => return Ok(image),
            0x02 if len == 2 => base = usize::from(u16::from_be_bytes([data[0], data[1]])) << 4,
            0x04 if len == 2 => base = usize::from(u16::from_be_bytes([data[0], data[1]])) << 16,
            0x03 | 0x05 => {} // start addresses mean nothing for AVR flash
            kind => return Err(at(&format!("unsupported record type {kind:02X}"))),
        }
    }
    Err("no end-of-file record".into())
}

fn record(out: &mut String, address: u16, kind: u8, data: &[u8]) {
    let [hi, lo] = address.to_be_bytes();
    let header = [data.len() as u8, hi, lo, kind];
    let checksum = sum(&header).wrapping_add(sum(data)).wrapping_neg();

    out.push(':');
    for byte in header.iter().chain(data) {
        let _ = write!(out, "{byte:02X}");
    }
    let _ = writeln!(out, "{checksum:02X}");
}

fn sum(bytes: &[u8]) -> u8 {
    bytes.iter().fold(0, |sum, b| sum.wrapping_add(*b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_what_it_encodes() {
        let image: Vec<u8> = (0..100u8).collect();
        assert_eq!(decode(&encode(&image)).unwrap(), image);
    }

    #[test]
    fn decodes_gaps_and_extended_addresses() {
        let text = ":020002000102F9\n:020000040001F9\n:01000000AA55\n:00000001FF\n";
        let image = decode(text).unwrap();
        assert_eq!(&image[..4], &[0xFF, 0xFF, 0x01, 0x02]);
        assert_eq!(image.len(), 0x10001);
        assert_eq!(image[0x10000], 0xAA);
    }

    #[test]
    fn rejects_bad_checksums_and_truncated_files() {
        assert!(
            decode(":0100000000FE\n:00000001FF\n")
                .unwrap_err()
                .contains("checksum")
        );
        assert!(
            decode(":0100000000FF\n")
                .unwrap_err()
                .contains("end-of-file")
        );
    }

    fn rec(address: u16, kind: u8, data: &[u8]) -> String {
        let mut line = String::new();
        record(&mut line, address, kind, data);
        line
    }

    const END: &str = ":00000001FF\n";

    #[test]
    fn refuses_a_line_with_half_a_byte_or_too_few_bytes_to_be_a_record() {
        for bad in [
            ":00000001FF0",
            ":00000001F",
            ":00000001",
            ":0000",
            ":00",
            ":",
        ] {
            let err = decode(&format!("{bad}\n{END}")).unwrap_err();
            assert_eq!(err, "line 1: malformed record", "{bad}");
        }
    }

    #[test]
    fn says_which_line_is_bad() {
        let good = rec(0, 0x00, &[0x12]);
        for (bad, what) in [
            ("00000001FF", "doesn't start with ':'"),
            (":0000000zFF", "not hex"),
            (":0000000é0FF", "not hex"),
            (":+0000001FF", "not hex"),
            (":0200000012EC", "length doesn't match"),
            (":00000001FE", "bad checksum"),
            (":00000006FA", "unsupported record type 06"),
        ] {
            let err = decode(&format!("{good}\n{bad}\n{END}")).unwrap_err();
            assert_eq!(err, format!("line 3: {what}"), "{bad}");
        }
    }

    #[test]
    fn skips_blank_lines_and_takes_dos_line_endings() {
        let text = format!(
            "\r\n{}\r\n\r\n  {}\r\n",
            rec(0, 0x00, &[0x12]).trim(),
            END.trim()
        );
        assert_eq!(decode(&text).unwrap(), [0x12]);
    }

    #[test]
    fn stops_at_the_end_of_file_record() {
        let text = format!("{}{END}{}", rec(0, 0x00, &[0x12]), rec(1, 0x00, &[0x34]));
        assert_eq!(decode(&text).unwrap(), [0x12]);
    }

    #[test]
    fn an_extended_segment_address_is_sixteen_times_its_value() {
        let text = format!(
            "{}{}{END}",
            rec(0, 0x02, &[0x10, 0x00]),
            rec(0x0002, 0x00, &[0xAA, 0xBB])
        );
        let image = decode(&text).unwrap();
        assert_eq!(image.len(), 0x1_0004);
        assert!(image[..0x1_0002].iter().all(|&b| b == 0xFF), "the gap");
        assert_eq!(&image[0x1_0002..], &[0xAA, 0xBB]);

        let text = format!(
            "{}{}{END}",
            rec(0, 0x02, &[0x00, 0x03]),
            rec(0x0001, 0x00, &[0xCC])
        );
        let image = decode(&text).unwrap();
        assert_eq!(image.len(), 0x32);
        assert_eq!(image[0x31], 0xCC);
    }

    #[test]
    fn an_extended_linear_address_is_the_high_sixteen_bits() {
        let text = format!(
            "{}{}{END}",
            rec(0, 0x04, &[0x00, 0x02]),
            rec(0x0010, 0x00, &[0xDD])
        );
        let image = decode(&text).unwrap();
        assert_eq!(image.len(), 0x2_0011);
        assert_eq!(image[0x2_0010], 0xDD);
    }

    #[test]
    fn a_base_address_lasts_until_the_next_one_which_replaces_it() {
        let text = format!(
            "{}{}{}{}{}{}{END}",
            rec(0x0000, 0x00, &[0x01]),
            rec(0, 0x02, &[0x00, 0x01]),
            rec(0x0000, 0x00, &[0x02]),
            rec(0x0001, 0x00, &[0x03]),
            rec(0, 0x04, &[0x00, 0x00]),
            rec(0x0002, 0x00, &[0x04]),
        );
        let image = decode(&text).unwrap();
        assert_eq!(image.len(), 0x12);
        assert_eq!(&image[..3], &[0x01, 0xFF, 0x04]);
        assert_eq!(&image[0x10..], &[0x02, 0x03]);
    }

    #[test]
    fn refuses_an_address_record_that_is_not_two_bytes() {
        for kind in [0x02, 0x04] {
            for data in [&[][..], &[0x00], &[0x00, 0x00, 0x00]] {
                let text = format!("{}{}{END}", rec(0, kind, data), rec(0, 0x00, &[0x12]));
                let err = decode(&text).unwrap_err();
                assert_eq!(
                    err,
                    format!("line 1: unsupported record type {kind:02X}"),
                    "{} bytes",
                    data.len()
                );
            }
        }
    }

    #[test]
    fn ignores_start_address_records() {
        let text = format!(
            "{}{}{}{END}",
            rec(0, 0x03, &[0x00, 0x00, 0x00, 0x7E]),
            rec(0, 0x05, &[0x00, 0x00, 0x00, 0x7E]),
            rec(0, 0x00, &[0x12]),
        );
        assert_eq!(decode(&text).unwrap(), [0x12]);
    }

    #[test]
    fn records_can_come_in_any_order_and_the_later_one_wins_an_overlap() {
        let text = format!(
            "{}{}{}{END}",
            rec(0x0004, 0x00, &[0x44, 0x55, 0x66]),
            rec(0x0000, 0x00, &[0x00, 0x11]),
            rec(0x0001, 0x00, &[0xA1, 0xA2, 0xA3, 0xA4]),
        );
        assert_eq!(
            decode(&text).unwrap(),
            [0x00, 0xA1, 0xA2, 0xA3, 0xA4, 0x55, 0x66]
        );
    }

    #[test]
    fn a_record_that_stops_where_the_image_does_leaves_its_length_alone() {
        let text = format!(
            "{}{}{END}",
            rec(0x0000, 0x00, &[0x00, 0x11, 0x22, 0x33]),
            rec(0x0002, 0x00, &[0xB2, 0xB3]),
        );
        assert_eq!(decode(&text).unwrap(), [0x00, 0x11, 0xB2, 0xB3]);
        let text = format!(
            "{}{}{END}",
            rec(0x0000, 0x00, &[0x00, 0x11, 0x22, 0x33]),
            rec(0x0003, 0x00, &[0xB3, 0xB4]),
        );
        assert_eq!(decode(&text).unwrap(), [0x00, 0x11, 0x22, 0xB3, 0xB4]);
    }

    #[test]
    fn empty_is_just_the_end_record() {
        assert_eq!(encode(&[]), ":00000001FF\n");
    }

    #[test]
    fn splits_into_sixteen_byte_lines_with_checksums() {
        let mut image = vec![0x0C, 0x94, 0x1F, 0x00];
        image.extend([0xFF; 14]);
        assert_eq!(
            encode(&image),
            ":100000000C941F00FFFFFFFFFFFFFFFFFFFFFFFF3D\n\
             :02001000FFFFF0\n\
             :00000001FF\n"
        );
    }
}
