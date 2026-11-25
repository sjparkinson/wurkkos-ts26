//! A text stamp of the commit, just before the CRC, so a `backup` can say
//! which build was on a torch. The firmware never reads it.

use std::path::Path;
use std::process::Command;

use crate::crc::STAMP_AT;

pub const STAMP_LEN: usize = 16;

const PREFIX: &[u8] = b"ts26 ";
const ROOM: usize = STAMP_LEN - PREFIX.len();

pub const TEST: [u8; STAMP_LEN] = *b"ts26 test\xFF\xFF\xFF\xFF\xFF\xFF\xFF";

pub fn format(hash: &str, dirty: bool) -> [u8; STAMP_LEN] {
    let mut stamp = [0xFF; STAMP_LEN];
    let room = if dirty { ROOM - 1 } else { ROOM };
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

/// `ts26 unknown` if git can't say, since a build shouldn't fail over a label.
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
    from_git(
        git(&["rev-parse", "--short=10", "HEAD"]).as_deref(),
        git(&["status", "--porcelain", "--untracked-files=no"]).as_deref(),
    )
}

fn from_git(hash: Option<&str>, status: Option<&str>) -> [u8; STAMP_LEN] {
    match (hash.filter(|hash| !hash.is_empty()), status) {
        (Some(hash), Some(status)) => format(hash, !status.is_empty()),
        _ => format("unknown", false),
    }
}

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

        let long = format("0123456789abcdef", true);
        assert_eq!(&long[..], b"ts26 0123456789+");

        let short = format("abc", false);
        assert_eq!(&short[..8], b"ts26 abc");
        assert!(short[8..].iter().all(|&b| b == 0xFF));
    }

    fn text(stamp: &[u8; STAMP_LEN]) -> String {
        let end = stamp.iter().position(|&b| b == 0xFF).unwrap_or(STAMP_LEN);
        String::from_utf8(stamp[..end].to_vec()).unwrap()
    }

    #[test]
    fn a_clean_tree_is_stamped_with_its_hash_and_a_dirty_one_with_a_plus() {
        assert_eq!(
            text(&from_git(Some("abc1234567"), Some(""))),
            "ts26 abc1234567"
        );
        assert_eq!(
            text(&from_git(Some("abc1234567"), Some(" M src/main.rs"))),
            "ts26 abc1234567+"
        );
    }

    #[test]
    fn a_build_git_cannot_describe_is_stamped_unknown_and_never_dirty() {
        for (hash, status) in [
            (None, None),
            (None, Some("")),
            (None, Some(" M src/main.rs")),
            (Some("abc1234567"), None),
            (Some(""), Some("")),
            (Some(""), Some(" M src/main.rs")),
        ] {
            assert_eq!(
                text(&from_git(hash, status)),
                "ts26 unknown",
                "{hash:?}, {status:?}"
            );
        }
    }

    /// Its `.git` is a file that names no repository, so git won't look
    /// above it for one.
    fn not_a_checkout() -> crate::TempDir {
        let dir = crate::TempDir::new("xtask-not-a-checkout").unwrap();
        std::fs::write(dir.path().join(".git"), "not a repository\n").unwrap();
        dir
    }

    #[test]
    fn a_build_outside_a_git_checkout_is_stamped_unknown() {
        let dir = not_a_checkout();
        assert_eq!(text(&current(dir.path())), "ts26 unknown");
        assert_eq!(text(&current(&dir.path().join("missing"))), "ts26 unknown");
    }

    #[test]
    fn the_stamp_for_this_checkout_is_a_hash_or_unknown() {
        let stamp = text(&current(&crate::root()));
        let name = stamp.strip_prefix("ts26 ").expect("the prefix");
        if name != "unknown" {
            let hash = name.strip_suffix('+').unwrap_or(name);
            assert_eq!(hash.len(), 10, "{stamp}");
            assert!(hash.bytes().all(|b| b.is_ascii_hexdigit()), "{stamp}");
        }
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
        let mut short = vec![0xFF; STAMP_AT + STAMP_LEN - 1];
        short[STAMP_AT..STAMP_AT + PREFIX.len()].copy_from_slice(PREFIX);
        assert_eq!(read(&short), None);
    }
}
