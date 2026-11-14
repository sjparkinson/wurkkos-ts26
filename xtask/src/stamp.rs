//! A short text stamp of the commit, kept in flash just before the CRC.
//!
//! It's there so a `backup` can say which build was on a torch. The firmware
//! never reads it. The CRC covers it (`crc.rs`), so a damaged stamp fails the
//! power-on scan like any other bad byte.
//!
//! The stamp reads `ts26 <hash>`, with a `+` after the hash if the tree had
//! uncommitted changes to tracked files, padded to [`STAMP_LEN`] bytes with
//! 0xFF, which is what erased flash reads as. A build outside a git checkout,
//! or one where git fails, says `ts26 unknown`.

use std::path::Path;
use std::process::Command;

use crate::crc::STAMP_AT;

/// How many bytes the stamp takes, the padding included.
pub const STAMP_LEN: usize = 16;

/// What every stamp starts with, and what `read` looks for.
const PREFIX: &[u8] = b"ts26 ";

/// A fixed stamp for the simulator and the tests, so neither depends on git.
pub const TEST: [u8; STAMP_LEN] = *b"ts26 test\xFF\xFF\xFF\xFF\xFF\xFF\xFF";

/// `ts26 <hash>`, then `+` if `dirty`, padded with 0xFF. A hash too long to
/// fit is cut short.
pub fn format(hash: &str, dirty: bool) -> [u8; STAMP_LEN] {
    let mut stamp = [0xFF; STAMP_LEN];
    let room = STAMP_LEN - PREFIX.len() - usize::from(dirty);
    let text = PREFIX
        .iter()
        .copied()
        .chain(hash.bytes().take(room))
        .chain(dirty.then_some(b'+'));
    for (slot, byte) in stamp.iter_mut().zip(text) {
        *slot = byte;
    }
    stamp
}

/// The stamp for the commit checked out in `root`. If git can't say, that's
/// `ts26 unknown`, since a build shouldn't fail over a label.
pub fn current(root: &Path) -> [u8; STAMP_LEN] {
    let git = |args: &[&str]| -> Option<String> {
        let out = Command::new("git")
            .current_dir(root)
            .args(args)
            .output()
            .ok()?;
        out.status
            .success()
            .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
    };
    let hash = git(&["rev-parse", "--short=10", "HEAD"]).filter(|hash| !hash.is_empty());
    let dirty = git(&["status", "--porcelain", "--untracked-files=no"]);
    match (hash, dirty) {
        (Some(hash), Some(dirty)) => format(&hash, !dirty.is_empty()),
        _ => format("unknown", false),
    }
}

/// The stamp text found in `flash`, or `None` if the bytes at the stamp's
/// place don't start with `ts26 `. That's erased flash, the stock firmware,
/// T0 (first light), or an image too short to reach it.
pub fn read(flash: &[u8]) -> Option<String> {
    let bytes = flash.get(STAMP_AT..STAMP_AT + STAMP_LEN)?;
    if !bytes.starts_with(PREFIX) {
        return None;
    }
    let text: Vec<u8> = bytes
        .iter()
        .copied()
        .take_while(|&b| b != 0xFF)
        .map(|b| {
            if b.is_ascii_graphic() || b == b' ' {
                b
            } else {
                b'?'
            }
        })
        .collect();
    Some(String::from_utf8_lossy(&text).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crc;

    #[test]
    fn formats_a_clean_and_a_dirty_commit() {
        let clean = format("0123456789", false);
        assert_eq!(&clean[..15], b"ts26 0123456789");
        assert_eq!(clean[15], 0xFF);

        let dirty = format("0123456789", true);
        assert_eq!(&dirty[..16], b"ts26 0123456789+");

        // A long hash is cut to fit, and the plus survives.
        let long = format("0123456789abcdef", true);
        assert_eq!(&long[..], b"ts26 0123456789+");

        let short = format("abc", false);
        assert_eq!(&short[..8], b"ts26 abc");
        assert!(short[8..].iter().all(|&b| b == 0xFF));
    }

    #[test]
    fn reads_back_what_it_writes() {
        for (hash, dirty, want) in [
            ("abc1234567", false, "ts26 abc1234567"),
            ("abc1234567", true, "ts26 abc1234567+"),
            ("unknown", false, "ts26 unknown"),
        ] {
            let flash = crc::sign(&[0x12, 0x34], &format(hash, dirty)).unwrap();
            assert_eq!(read(&flash).as_deref(), Some(want));
        }
        let flash = crc::sign(&[], &TEST).unwrap();
        assert_eq!(read(&flash).as_deref(), Some("ts26 test"));
    }

    #[test]
    fn erased_flash_has_no_stamp() {
        assert_eq!(read(&vec![0xFF; crc::CHECKSUM_AT + 2]), None);
    }

    #[test]
    fn a_short_image_has_no_stamp() {
        assert_eq!(read(&[]), None);
        assert_eq!(read(&[0x12; 628]), None);
        // One byte short of reaching the end of the stamp.
        let mut short = vec![0xFF; STAMP_AT + STAMP_LEN - 1];
        short[STAMP_AT..STAMP_AT + PREFIX.len()].copy_from_slice(PREFIX);
        assert_eq!(read(&short), None);
    }
}
