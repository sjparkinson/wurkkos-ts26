//! Checks on the linked firmware for things src/avr/rt.rs, src/avr/hw.rs and
//! link.x assume but no compiler or linker error would catch. That's the
//! vector table's shape, where the startup code copies and zeroes memory, and
//! that every unlock of a protected register is followed straight away by the
//! write. And that nothing in the firmware can reach a panic, since the
//! handler in src/main.rs is there because Rust requires one.
//!
//! `build()` in main.rs runs these, so `size`, `hex` and `flash` all refuse
//! an image that fails, and CI runs them through `size`.
//!
//! The opcodes come from Microchip's AVR Instruction Set Manual. Only the
//! handful the checks look for are decoded.

use ts26::regs::{VECTORS, addr};

use crate::avr::is_two_words;
use crate::elf::{DATA_SPACE, Elf, RAM, Segment};
use crate::updi::map;

/// CPU.CCP's I/O address, the same number as its data-space one.
const CPU_CCP: u16 = addr::CPU_CCP as u16;

/// Where tinyAVR maps flash into data space, and how much of it there is.
const MAPPED_FLASH: u32 = map::FLASH as u32;
const FLASH_SIZE: u32 = map::FLASH_SIZE as u32;
/// `eor r1, r1`. `eor Rd, Rr` is 0010 01rd dddd rrrr.
const EOR_R1_R1: u16 = 0x2411;

pub fn check(elf: &Elf) -> Result<(), String> {
    memory(elf)?;
    let text = elf.section(".text").ok_or("no .text section")?;
    let image = elf.flash_image();
    let text = image
        .get(text.start as usize..text.end as usize)
        .ok_or(".text isn't in the flash image")?;
    protected_writes(text)?;
    no_panics(elf.symbols())
}

/// LTO drops the panic machinery while nothing reaches it, so a symbol with
/// one of these in its name means an `[i]`, an `unwrap` or the like now does.
/// The names are `core::panicking::*`, `core::slice::index::*_fail` (which
/// call into `panicking`) and the `rust_begin_unwind` the panic handler is
/// linked as. They're what this nightly emits, so re-prove the check fires
/// when bumping it (CONTRIBUTING.md).
const PANIC_PATTERNS: [&str; 2] = ["panic", "begin_unwind"];

/// Refuses an image with a panic or a bounds check linked in.
fn no_panics<'a>(symbols: impl IntoIterator<Item = &'a str>) -> Result<(), String> {
    let found: Vec<&str> = symbols
        .into_iter()
        .filter(|name| PANIC_PATTERNS.iter().any(|p| name.contains(p)))
        .collect();
    if found.is_empty() {
        return Ok(());
    }
    Err(format!(
        "the firmware links panic machinery ({}), so something in it can panic and reach hw::halt. \
         Find the caller with `llvm-objdump -d target/avr-none/release/ts26.elf` (the llvm tools \
         are under ~/.rustup/toolchains/<nightly>/lib/rustlib/*/bin/) or the avr-codegen agent",
        found.join(", ")
    ))
}

/// The checks that are about link.x and src/avr/rt.rs rather than the
/// firmware, which the link probe (examples/link.rs) gets too.
pub fn memory(elf: &Elf) -> Result<(), String> {
    let symbol = |name| {
        elf.symbol(name)
            .ok_or_else(|| format!("no `{name}` symbol"))
    };
    let reset = symbol("__reset")?;
    if elf.entry() != reset {
        return Err(format!(
            "entry point is {:#06x}, but __reset is at {reset:#06x}",
            elf.entry()
        ));
    }

    let image = elf.flash_image();
    vectors(
        &image,
        reset,
        symbol("__unhandled_interrupt")?,
        halt(elf.symbols(), |name| elf.symbol(name))?,
    )?;

    let ram_layout = Ram {
        data_start: symbol("__data_start")?,
        data_end: symbol("__data_end")?,
        bss_start: symbol("__bss_start")?,
        bss_end: symbol("__bss_end")?,
        data_load: symbol("__data_load")?,
    };
    ram(&ram_layout, elf.segments())?;
    mapped(elf.segments())
}

/// What a symbol for `hw::halt` has in its name. It isn't `no_mangle`, so the
/// name is mangled, but both of Rust's schemes spell each path segment as its
/// length then its name. The firmware's is `ts26::avr::hw::halt` and the link
/// probe's `link::hw::halt`.
const HALT: &str = "2hw4halt";

/// The byte address of `hw::halt`, the one symbol with `HALT` in its name.
/// `value` looks a symbol up by name.
pub(crate) fn halt<'a>(
    names: impl IntoIterator<Item = &'a str>,
    value: impl Fn(&str) -> Option<u32>,
) -> Result<u32, String> {
    let found: Vec<&str> = names.into_iter().filter(|n| n.contains(HALT)).collect();
    match found[..] {
        [name] => value(name).ok_or_else(|| format!("no value for `{name}`")),
        [] => Err("no `hw::halt` symbol".into()),
        _ => Err(format!(
            "more than one symbol looks like `hw::halt`: {}",
            found.join(", ")
        )),
    }
}

/// Every slot is a `jmp`. Slot 0 goes to `__reset`, the rest somewhere in
/// flash past the table. `__unhandled_interrupt` is `eor r1, r1` then a `jmp`
/// to `halt`, src/avr/rt.rs's way of going dark rather than returning into
/// an interrupt that would fire again forever.
fn vectors(image: &[u8], reset: u32, unhandled: u32, halt: u32) -> Result<(), String> {
    let table_end = (VECTORS * 4) as u32;
    for slot in 0..VECTORS {
        let target = jmp_target(image, slot * 4)
            .ok_or_else(|| format!("vector {slot} at {:#06x} isn't a jmp", slot * 4))?;
        if slot == 0 && target != reset {
            return Err(format!(
                "the reset vector jumps to {target:#06x}, but __reset is at {reset:#06x}"
            ));
        }
        if target < table_end || target as usize >= image.len() {
            return Err(format!(
                "vector {slot} jumps to {target:#06x}, outside the code after the table"
            ));
        }
    }
    if word(image, unhandled as usize) != Some(EOR_R1_R1) {
        return Err(format!(
            "__unhandled_interrupt at {unhandled:#06x} doesn't start with `eor r1, r1`"
        ));
    }
    let target = jmp_target(image, unhandled as usize + 2).ok_or_else(|| {
        format!("__unhandled_interrupt at {unhandled:#06x} doesn't `jmp` after clearing r1")
    })?;
    if target < table_end || target as usize >= image.len() || target != halt {
        return Err(format!(
            "__unhandled_interrupt jumps to {target:#06x}, but hw::halt is at {halt:#06x}"
        ));
    }
    Ok(())
}

/// The symbols src/avr/rt.rs's startup loops use, from link.x.
struct Ram {
    data_start: u32,
    data_end: u32,
    bss_start: u32,
    bss_end: u32,
    /// A data-space address in the mapped-flash window, not an ELF one.
    data_load: u32,
}

/// .data then .bss, in order, inside SRAM. The loops in src/avr/rt.rs stop
/// when the pointer equals the end, so an end below its start would run through
/// all of data space. And .data's initial values have to sit where
/// `__data_load` says, or the copy fills SRAM with the wrong bytes.
fn ram(r: &Ram, segments: &[Segment]) -> Result<(), String> {
    let order = [
        (RAM.start, "the start of SRAM"),
        (r.data_start, "__data_start"),
        (r.data_end, "__data_end"),
        (r.bss_start, "__bss_start"),
        (r.bss_end, "__bss_end"),
        (RAM.end, "the end of SRAM"),
    ];
    for pair in order.windows(2) {
        let [(a, a_name), (b, b_name)] = pair else {
            unreachable!()
        };
        if a > b {
            return Err(format!("{a_name} ({a:#08x}) is after {b_name} ({b:#08x})"));
        }
    }

    let len = r.data_end - r.data_start;
    if len == 0 {
        return Ok(());
    }
    let window = MAPPED_FLASH..MAPPED_FLASH + FLASH_SIZE;
    if !window.contains(&r.data_load) || !window.contains(&(r.data_load + len - 1)) {
        return Err(format!(
            "__data_load ({:#06x}) isn't in the mapped-flash window",
            r.data_load
        ));
    }
    let data = segments
        .iter()
        .find(|s| s.vaddr == r.data_start)
        .ok_or("no loadable segment holds .data's initial values")?;
    if data.paddr + MAPPED_FLASH != r.data_load || data.data.len() as u32 != len {
        return Err(format!(
            "__data_load is {:#06x}, but .data's {} bytes are at flash {:#06x} ({} bytes)",
            r.data_load,
            len,
            data.paddr,
            data.data.len()
        ));
    }
    Ok(())
}

/// Code is linked where it's loaded, and anything linked in the mapped-flash
/// window (.rodata) at its flash address plus the window's ELF address, so
/// an ordinary load finds it.
fn mapped(segments: &[Segment]) -> Result<(), String> {
    let elf_window = DATA_SPACE + MAPPED_FLASH;
    for s in segments {
        let expected = if s.vaddr < DATA_SPACE {
            s.paddr
        } else if (elf_window..elf_window + FLASH_SIZE).contains(&s.vaddr) {
            s.paddr + elf_window
        } else {
            continue; // .data, checked by `ram`
        };
        if s.vaddr != expected {
            return Err(format!(
                "segment at flash {:#06x} is linked at {:#08x}, expected {expected:#08x}",
                s.paddr, s.vaddr
            ));
        }
    }
    Ok(())
}

/// Every `out CCP, rN` is followed at once by `st Z, rM`, the pairing
/// `protected_write` in src/avr/hw.rs relies on, because the chip only accepts
/// the write within four instructions of the key. Also insists there's at
/// least one, since `hw::init` sets the clock prescaler that way.
fn protected_writes(text: &[u8]) -> Result<usize, String> {
    let mut found = 0;
    let mut at = 0;
    while let Some(w) = word(text, at) {
        if out_address(w) == Some(CPU_CCP) {
            if !word(text, at + 2).is_some_and(is_st_z) {
                return Err(format!(
                    "the CCP unlock at {at:#06x} isn't followed by `st Z`"
                ));
            }
            found += 1;
        }
        at += if is_two_words(w) { 4 } else { 2 };
    }
    if found == 0 {
        return Err("no CCP unlock in .text, but hw::init should set the clock through one".into());
    }
    Ok(found)
}

/// The little-endian word at byte `at`, if there's one there.
pub(crate) fn word(bytes: &[u8], at: usize) -> Option<u16> {
    let b = bytes.get(at..at + 2)?;
    Some(u16::from_le_bytes([b[0], b[1]]))
}

/// The byte address a `jmp` at `at` goes to. `jmp k` is
/// 1001 010k kkkk 110k, then the low 16 bits of k, a word address.
pub(crate) fn jmp_target(image: &[u8], at: usize) -> Option<u32> {
    let (w0, w1) = (word(image, at)?, word(image, at + 2)?);
    if w0 & 0xFE0E != 0x940C {
        return None;
    }
    let high = u32::from(w0 & 0x01F0) << 13 | u32::from(w0 & 1) << 16;
    Some((high | u32::from(w1)) * 2)
}

/// The I/O address an `out A, Rr` writes. The encoding is 1011 1AAr rrrr AAAA.
fn out_address(w: u16) -> Option<u16> {
    (w & 0xF800 == 0xB800).then_some((w >> 5) & 0x30 | w & 0x0F)
}

/// `st Z, Rr` is 1000 001r rrrr 0000.
const fn is_st_z(w: u16) -> bool {
    w & 0xFE0F == 0x8200
}

#[cfg(test)]
mod tests {
    use super::*;

    const OUT_CCP_R24: u16 = 0xBF84; // out 0x34, r24
    const ST_Z_R25: u16 = 0x8390; // st Z, r25
    const NOP: u16 = 0x0000;

    fn bytes(words: &[u16]) -> Vec<u8> {
        words.iter().flat_map(|w| w.to_le_bytes()).collect()
    }

    fn jmp(byte_addr: u32) -> [u16; 2] {
        let k = byte_addr / 2;
        let w0 = 0x940C | ((k >> 13) & 0x01F0) as u16 | ((k >> 16) & 1) as u16;
        [w0, k as u16]
    }

    /// Where `image` puts `__unhandled_interrupt`, reset and `hw::halt`.
    const UNHANDLED: u32 = 0x7C;
    const RESET: u32 = 0x82;
    const HALT_AT: u32 = 0x84;
    const RETI: u16 = 0x9518;

    /// `__unhandled_interrupt` as src/avr/rt.rs has it.
    fn clear_r1_and_halt() -> [u16; 3] {
        let [w0, w1] = jmp(HALT_AT);
        [EOR_R1_R1, w0, w1]
    }

    /// A vector table, then `unhandled` at 0x7C, and a `nop` each for reset
    /// at 0x82 and `hw::halt` at 0x84.
    fn image_with(table: impl Fn(usize) -> [u16; 2], unhandled: [u16; 3]) -> Vec<u8> {
        let mut words: Vec<u16> = (0..VECTORS).flat_map(table).collect();
        words.extend(unhandled);
        words.extend([NOP, NOP]);
        bytes(&words)
    }

    fn image(table: impl Fn(usize) -> [u16; 2]) -> Vec<u8> {
        image_with(table, clear_r1_and_halt())
    }

    fn standard(slot: usize) -> [u16; 2] {
        jmp(if slot == 0 { RESET } else { UNHANDLED })
    }

    #[test]
    fn decodes_jmp_across_the_whole_address_range() {
        for addr in [0, 0x7C, 0x3FFE, 0x2_0000, 0x7F_FFFE] {
            assert_eq!(jmp_target(&bytes(&jmp(addr)), 0), Some(addr));
        }
        assert_eq!(jmp_target(&bytes(&[0x940E, 0]), 0), None, "call, not jmp");
    }

    #[test]
    fn accepts_a_well_formed_vector_table() {
        assert_eq!(vectors(&image(standard), RESET, UNHANDLED, HALT_AT), Ok(()));
    }

    #[test]
    fn rejects_a_reset_vector_pointing_elsewhere() {
        let img = image(|_| jmp(UNHANDLED));
        let err = vectors(&img, RESET, UNHANDLED, HALT_AT).unwrap_err();
        assert!(err.contains("reset vector"), "{err}");
    }

    #[test]
    fn rejects_a_short_table() {
        // One slot short, so the last slot is `eor r1, r1` and half a `jmp`.
        let mut img = image(standard);
        img.drain(..4);
        assert!(vectors(&img, RESET - 4, UNHANDLED - 4, HALT_AT - 4).is_err());
    }

    #[test]
    fn rejects_a_vector_into_the_table_or_past_the_image() {
        for bad in [0x10, 0x1000] {
            let img = image(|slot| if slot == 5 { jmp(bad) } else { standard(slot) });
            let err = vectors(&img, RESET, UNHANDLED, HALT_AT).unwrap_err();
            assert!(err.contains("vector 5"), "{err}");
        }
    }

    #[test]
    fn accepts_an_unhandled_interrupt_that_clears_r1_and_jumps_to_halt() {
        let img = image_with(standard, clear_r1_and_halt());
        assert_eq!(word(&img, UNHANDLED as usize), Some(EOR_R1_R1));
        assert_eq!(jmp_target(&img, UNHANDLED as usize + 2), Some(HALT_AT));
        assert_eq!(vectors(&img, RESET, UNHANDLED, HALT_AT), Ok(()));
    }

    #[test]
    fn rejects_an_unhandled_interrupt_that_doesnt_clear_r1_and_jump() {
        let [_, w0, w1] = clear_r1_and_halt();
        let to = |addr| {
            let [w0, w1] = jmp(addr);
            [EOR_R1_R1, w0, w1]
        };
        let cases = [
            ([RETI, NOP, NOP], "eor r1, r1"),
            ([NOP, w0, w1], "eor r1, r1"),
            ([0x2400, w0, w1], "eor r1, r1"), // eor r0, r0
            ([EOR_R1_R1, RETI, NOP], "doesn't `jmp`"),
            ([EOR_R1_R1, 0x940E, w1], "doesn't `jmp`"), // call, not jmp
            (to(RESET), "hw::halt is at"),
            (to(0x10), "hw::halt is at"),
            (to(0x1000), "hw::halt is at"),
        ];
        for (unhandled, expected) in cases {
            let img = image_with(standard, unhandled);
            let err = vectors(&img, RESET, UNHANDLED, HALT_AT).unwrap_err();
            assert!(err.contains(expected), "{unhandled:04x?}: {err}");
        }
    }

    #[test]
    fn finds_halt_by_its_mangled_path() {
        let value = |name: &str| name.starts_with("_ZN").then_some(0x1234);
        let firmware = [
            "__reset",
            "_ZN3ts263avr2hw8set_output17h0123456789abcdefE",
            "_ZN3ts263avr2hw4halt17h0123456789abcdefE",
        ];
        assert_eq!(halt(firmware, value), Ok(0x1234));
        let probe = ["main", "_ZN4link2hw4halt17h0123456789abcdefE"];
        assert_eq!(halt(probe, value), Ok(0x1234));
        let v0 = ["_RNvNtNtCs1234_4ts263avr2hw4halt"];
        assert_eq!(halt(v0, |_| Some(0x42)), Ok(0x42));

        let none = ["main", "_ZN3ts263avr2hw8set_output17h0123456789abcdefE"];
        assert!(halt(none, value).unwrap_err().contains("no `hw::halt`"));
        let two = [
            "_ZN3ts263avr2hw4halt17h0123456789abcdefE",
            "_ZN3ts263avr2hw4halt17hfedcba9876543210E",
        ];
        assert!(halt(two, value).unwrap_err().contains("more than one"));
    }

    fn ram_at(data: u32, data_len: u32, bss_len: u32, data_load: u32) -> Ram {
        Ram {
            data_start: data,
            data_end: data + data_len,
            bss_start: data + data_len,
            bss_end: data + data_len + bss_len,
            data_load,
        }
    }

    fn segment(vaddr: u32, paddr: u32, len: usize) -> Segment {
        Segment {
            vaddr,
            paddr,
            data: vec![0; len],
        }
    }

    #[test]
    fn accepts_empty_data_and_bss() {
        assert_eq!(ram(&ram_at(RAM.start, 0, 0, 0x8252), &[]), Ok(()));
    }

    #[test]
    fn accepts_data_loaded_where_data_load_says() {
        let segs = [segment(RAM.start, 0x252, 4)];
        assert_eq!(ram(&ram_at(RAM.start, 4, 8, 0x8252), &segs), Ok(()));
    }

    #[test]
    fn rejects_data_load_off_by_some_bytes() {
        let segs = [segment(RAM.start, 0x250, 4)];
        assert!(ram(&ram_at(RAM.start, 4, 8, 0x8252), &segs).is_err());
    }

    #[test]
    fn rejects_data_load_outside_the_mapped_window() {
        let segs = [segment(RAM.start, 0x252, 4)];
        let err = ram(&ram_at(RAM.start, 4, 0, 0x0252), &segs).unwrap_err();
        assert!(err.contains("mapped-flash"), "{err}");
    }

    #[test]
    fn rejects_sections_out_of_order_or_outside_sram() {
        let mut r = ram_at(RAM.start, 0, 4, 0x8252);
        r.bss_end = r.bss_start - 1;
        assert!(ram(&r, &[]).unwrap_err().contains("__bss_start"));
        assert!(ram(&ram_at(RAM.end - 2, 0, 4, 0x8252), &[]).is_err());
        assert!(ram(&ram_at(RAM.start - 2, 0, 0, 0x8252), &[]).is_err());
    }

    #[test]
    fn checks_rodata_is_linked_at_its_flash_address_in_the_window() {
        let good = [segment(0, 0, 4), segment(0x80_8100, 0x100, 2)];
        assert_eq!(mapped(&good), Ok(()));
        let bad = [segment(0x80_8000, 0x100, 2)];
        assert!(mapped(&bad).is_err());
        let text_moved = [segment(0x10, 0, 4)];
        assert!(mapped(&text_moved).is_err());
        let data = [segment(0x80_3800, 0x200, 2)];
        assert_eq!(mapped(&data), Ok(()), ".data is `ram`'s job");
    }

    #[test]
    fn decodes_out_and_st_z() {
        assert_eq!(out_address(OUT_CCP_R24), Some(CPU_CCP));
        assert_eq!(out_address(0xBE1F), Some(0x3F), "out SREG, r1");
        assert_eq!(out_address(ST_Z_R25), None);
        assert!(is_st_z(ST_Z_R25));
        assert!(!is_st_z(0x8391), "std Z+1, not st Z");
    }

    #[test]
    fn accepts_an_unlock_followed_by_the_write() {
        let text = bytes(&[NOP, OUT_CCP_R24, ST_Z_R25, NOP, OUT_CCP_R24, ST_Z_R25]);
        assert_eq!(protected_writes(&text), Ok(2));
    }

    #[test]
    fn rejects_an_unlock_with_anything_in_between() {
        let text = bytes(&[OUT_CCP_R24, NOP, ST_Z_R25]);
        assert!(protected_writes(&text).unwrap_err().contains("0x0000"));
        let text = bytes(&[NOP, OUT_CCP_R24]);
        assert!(protected_writes(&text).is_err(), "unlock at the very end");
    }

    #[test]
    fn skips_the_address_word_of_two_word_instructions() {
        // `lds r24, 0xBF84` has an address that happens to encode `out 0x34,
        // r24`.
        let text = bytes(&[0x9180, OUT_CCP_R24, NOP, OUT_CCP_R24, ST_Z_R25]);
        assert_eq!(protected_writes(&text), Ok(1));
        // And a `call` whose address does the same.
        let text = bytes(&[0x940E, OUT_CCP_R24, OUT_CCP_R24, ST_Z_R25]);
        assert_eq!(protected_writes(&text), Ok(1));
    }

    #[test]
    fn insists_on_at_least_one_protected_write() {
        assert!(protected_writes(&bytes(&[NOP, ST_Z_R25])).is_err());
    }

    #[test]
    fn accepts_symbols_with_no_panic_machinery() {
        let symbols = [
            "__reset",
            "__unhandled_interrupt",
            "main",
            "_ZN3ts266torch5Torch4tick17h0123456789abcdefE",
            "_ZN3ts263avr2hw4halt17h0123456789abcdefE",
        ];
        assert_eq!(no_panics(symbols), Ok(()));
    }

    #[test]
    fn rejects_a_panic_handler_or_a_bounds_check() {
        let panicking = [
            "main",
            "_ZN4core9panicking18panic_bounds_check17h0123456789abcdefE",
        ];
        let err = no_panics(panicking).unwrap_err();
        assert!(err.contains("panic_bounds_check"), "{err}");
        assert!(err.contains("llvm-objdump"), "{err}");
        assert!(err.contains("avr-codegen"), "{err}");

        let handler = ["rust_begin_unwind", "main"];
        assert!(
            no_panics(handler)
                .unwrap_err()
                .contains("rust_begin_unwind")
        );

        let both = [
            "_ZN4core9panicking5panic17h0123456789abcdefE",
            "rust_begin_unwind",
        ];
        let err = no_panics(both).unwrap_err();
        assert!(
            err.contains("5panic") && err.contains("rust_begin_unwind"),
            "{err}"
        );
    }
}
