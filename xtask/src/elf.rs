use std::ffi::CStr;
use std::ops::Range;

use ts26::regs::mem;

use crate::int;

/// AVR ELF files put data space at 0x800000.
pub const DATA_SPACE: u32 = 0x80_0000;
pub const RAM: Range<u32> = DATA_SPACE + int::u32_of(mem::INTERNAL_SRAM_START)
    ..DATA_SPACE + int::u32_of(mem::INTERNAL_SRAM_END);
pub const SRAM: Range<u16> =
    int::u16_of(mem::INTERNAL_SRAM_START)..int::u16_of(mem::INTERNAL_SRAM_END);

const PT_LOAD: u32 = 1;
const SHT_SYMTAB: u32 = 2;
const SHF_ALLOC: u32 = 0x2;
const EM_AVR: u16 = 83;

pub struct Elf {
    entry: u32,
    segments: Vec<Segment>,
    sections: Vec<Section>,
    symbols: Vec<(String, u32)>,
    image: Vec<u8>,
}

pub struct Segment {
    pub vaddr: u32,
    pub paddr: u32,
    pub data: Vec<u8>,
}

struct Section {
    name: String,
    flags: u32,
    addr: Range<u32>,
    size: u32,
}

impl Elf {
    pub fn parse(bytes: &[u8]) -> Result<Self, String> {
        let r = Reader {
            file: bytes,
            base: 0,
        };
        if bytes.get(..4) != Some(b"\x7fELF".as_slice()) {
            return Err("not an ELF file".into());
        }
        if r.u8(4)? != 1 || r.u8(5)? != 1 {
            return Err("not a 32-bit little-endian ELF file".into());
        }
        if r.u16(18)? != EM_AVR {
            return Err("not an AVR ELF file".into());
        }
        let entry = r.u32(24)?;

        let (phoff, phentsize, phnum) = (r.u32(28)?, r.u16(42)?, r.u16(44)?);
        let mut segments = Vec::new();
        for i in 0..phnum {
            let ph = r.at(phoff)?.entry(phentsize, i.into())?;
            let (kind, offset, vaddr, paddr, filesz) =
                (ph.u32(0)?, ph.u32(4)?, ph.u32(8)?, ph.u32(12)?, ph.u32(16)?);
            if kind == PT_LOAD && filesz > 0 && paddr < DATA_SPACE {
                let data = r.slice(offset, filesz)?.to_vec();
                segments.push(Segment { vaddr, paddr, data });
            }
        }

        let (shoff, shentsize, shnum, shstrndx) = (r.u32(32)?, r.u16(46)?, r.u16(48)?, r.u16(50)?);
        let header = |i: u32| r.at(shoff)?.entry(shentsize, i);
        let names = r.at(header(shstrndx.into())?.u32(16)?)?;
        let mut sections = Vec::new();
        let mut symbols = Vec::new();
        for i in 0..shnum {
            let sh = header(i.into())?;
            let name = names.cstr(sh.u32(0)?)?;
            let (flags, addr, size) = (sh.u32(8)?, sh.u32(12)?, sh.u32(20)?);
            let end = addr
                .checked_add(size)
                .ok_or_else(|| format!("section {name} ends past the 32-bit address space"))?;
            sections.push(Section {
                name,
                flags,
                addr: addr..end,
                size,
            });
            if sh.u32(4)? == SHT_SYMTAB {
                let (table, size) = (r.at(sh.u32(16)?)?, sh.u32(20)?);
                let strings = r.at(header(sh.u32(24)?)?.u32(16)?)?;
                for sym in (0..size).step_by(16).skip(1) {
                    let sym = table.at(sym)?;
                    symbols.push((strings.cstr(sym.u32(0)?)?, sym.u32(4)?));
                }
            }
        }

        let image = lay_out(&segments)?;
        Ok(Self {
            entry,
            segments,
            sections,
            symbols,
            image,
        })
    }

    pub fn flash_image(&self) -> Vec<u8> {
        self.image.clone()
    }

    pub fn ram_bytes(&self) -> u32 {
        self.sections
            .iter()
            .filter(|s| s.flags & SHF_ALLOC != 0 && RAM.contains(&s.addr.start))
            .map(|s| s.size)
            .sum()
    }

    pub const fn entry(&self) -> u32 {
        self.entry
    }

    pub fn segments(&self) -> &[Segment] {
        &self.segments
    }

    pub fn section(&self, name: &str) -> Option<Range<u32>> {
        self.sections
            .iter()
            .find(|s| s.name == name)
            .map(|s| s.addr.clone())
    }

    pub fn symbol(&self, name: &str) -> Option<u32> {
        self.symbols
            .iter()
            .find(|(n, _)| n == name)
            .map(|&(_, value)| value)
    }

    pub fn require(&self, name: &str) -> Result<u32, String> {
        self.symbol(name)
            .ok_or_else(|| format!("no `{name}` symbol"))
    }

    pub fn symbols(&self) -> impl Iterator<Item = &str> + '_ {
        self.symbols.iter().map(|(name, _)| name.as_str())
    }
}

fn lay_out(segments: &[Segment]) -> Result<Vec<u8>, String> {
    let mut image = Vec::new();
    for s in segments {
        let too_big = || format!("the segment at flash {:#06x} is too big", s.paddr);
        let start = usize::try_from(s.paddr).map_err(|_| too_big())?;
        let end = start.checked_add(s.data.len()).ok_or_else(too_big)?;
        if image.len() < end {
            image.resize(end, 0xFF);
        }
        for (to, &from) in image.iter_mut().skip(start).zip(&s.data) {
            *to = from;
        }
    }
    Ok(image)
}

#[derive(Clone, Copy)]
struct Reader<'a> {
    file: &'a [u8],
    base: usize,
}

impl<'a> Reader<'a> {
    fn position(self, offset: u32) -> Result<usize, String> {
        usize::try_from(offset)
            .ok()
            .and_then(|offset| self.base.checked_add(offset))
            .ok_or_else(|| format!("truncated at byte {offset} after byte {}", self.base))
    }

    fn at(self, offset: u32) -> Result<Self, String> {
        Ok(Self {
            file: self.file,
            base: self.position(offset)?,
        })
    }

    fn entry(self, size: u16, index: u32) -> Result<Self, String> {
        let offset = index.checked_mul(size.into()).ok_or_else(|| {
            format!(
                "truncated at entry {index} of the table at byte {}",
                self.base
            )
        })?;
        self.at(offset)
    }

    fn slice(self, offset: u32, len: u32) -> Result<&'a [u8], String> {
        let at = self.position(offset)?;
        usize::try_from(len)
            .ok()
            .and_then(|len| at.checked_add(len))
            .and_then(|end| self.file.get(at..end))
            .ok_or_else(|| format!("truncated at byte {at}"))
    }

    fn array<const N: usize>(self, offset: u32) -> Result<[u8; N], String> {
        let at = self.position(offset)?;
        self.file
            .get(at..)
            .and_then(<[u8]>::first_chunk)
            .copied()
            .ok_or_else(|| format!("truncated at byte {at}"))
    }

    fn u8(self, offset: u32) -> Result<u8, String> {
        self.array(offset).map(u8::from_le_bytes)
    }

    fn u16(self, offset: u32) -> Result<u16, String> {
        self.array(offset).map(u16::from_le_bytes)
    }

    fn u32(self, offset: u32) -> Result<u32, String> {
        self.array(offset).map(u32::from_le_bytes)
    }

    fn cstr(self, offset: u32) -> Result<String, String> {
        let at = self.position(offset)?;
        let rest = self
            .file
            .get(at..)
            .ok_or_else(|| format!("truncated at byte {at}"))?;
        let text = CStr::from_bytes_until_nul(rest)
            .map_err(|_| format!("unterminated string at byte {at}"))?;
        text.to_str()
            .map(str::to_owned)
            .map_err(|_| format!("bad string at byte {at}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Vec<u8> {
        sample_with(&[])
    }

    /// Each extra header is `(type, vaddr, paddr, filesz)`.
    fn sample_with(extra: &[(u32, u32, u32, u32)]) -> Vec<u8> {
        const SHNUM: usize = 5;
        let phnum = 2 + extra.len();
        let shoff = 52 + phnum * 32;
        let data_at = shoff + SHNUM * 40;

        let mut f = vec![0u8; 52];
        f[..4].copy_from_slice(b"\x7fELF");
        f[4] = 1; // 32-bit
        f[5] = 1; // little-endian
        f[18..20].copy_from_slice(&EM_AVR.to_le_bytes());
        f[24..28].copy_from_slice(&0x7Eu32.to_le_bytes()); // entry
        f[28..32].copy_from_slice(&52u32.to_le_bytes()); // phoff
        f[32..36].copy_from_slice(&(shoff as u32).to_le_bytes());
        f[42..44].copy_from_slice(&32u16.to_le_bytes()); // phentsize
        f[44..46].copy_from_slice(&(phnum as u16).to_le_bytes());
        f[46..48].copy_from_slice(&40u16.to_le_bytes()); // shentsize
        f[48..50].copy_from_slice(&(SHNUM as u16).to_le_bytes());
        f[50..52].copy_from_slice(&2u16.to_le_bytes()); // shstrndx

        let image = [0x0C, 0x94, 0x1F, 0x00, 0xAA, 0xBB];
        let shstrtab = b"\0.bss\0.shstrtab\0.symtab\0.strtab\0.debug_info\0";
        let strtab = b"\0__reset\0main\0halt\0";
        let mut symtab = vec![0u8; 16]; // null symbol
        for (name, value) in [(1u32, 0x7Eu32), (9, 0xB2), (14, 0xC4)] {
            let mut sym = vec![0u8; 16];
            sym[0..4].copy_from_slice(&name.to_le_bytes());
            sym[4..8].copy_from_slice(&value.to_le_bytes());
            symtab.extend(sym);
        }
        let image_at = data_at as u32;
        let shstrtab_at = image_at + image.len() as u32;
        let strtab_at = shstrtab_at + shstrtab.len() as u32;
        let symtab_at = strtab_at + strtab.len() as u32;

        let headers = [
            (PT_LOAD, 0u32, 0u32, image_at, 4u32),
            (PT_LOAD, 0x80_8006, 6, image_at + 4, 2),
        ];
        let extra = extra
            .iter()
            .map(|&(kind, vaddr, paddr, len)| (kind, vaddr, paddr, image_at, len));
        for (kind, vaddr, paddr, offset, len) in headers.into_iter().chain(extra) {
            let mut ph = vec![0u8; 32];
            ph[0..4].copy_from_slice(&kind.to_le_bytes());
            ph[4..8].copy_from_slice(&offset.to_le_bytes());
            ph[8..12].copy_from_slice(&vaddr.to_le_bytes());
            ph[12..16].copy_from_slice(&paddr.to_le_bytes());
            ph[16..20].copy_from_slice(&len.to_le_bytes());
            f.extend(ph);
        }
        // (name, type, flags, addr, offset, size, link)
        let sections = [
            (1u32, 8u32, SHF_ALLOC | 0x1, 0x80_3800u32, 0u32, 12u32, 0u32), // .bss counts
            (34, 1, 0, 0x80_3900, 0, 999, 0),
            (6, 3, 0, 0, shstrtab_at, shstrtab.len() as u32, 0),
            (16, SHT_SYMTAB, 0, 0, symtab_at, symtab.len() as u32, 4),
            (24, 3, 0, 0, strtab_at, strtab.len() as u32, 0),
        ];
        for (name, kind, flags, addr, offset, size, link) in sections {
            let mut sh = vec![0u8; 40];
            sh[0..4].copy_from_slice(&name.to_le_bytes());
            sh[4..8].copy_from_slice(&kind.to_le_bytes());
            sh[8..12].copy_from_slice(&flags.to_le_bytes());
            sh[12..16].copy_from_slice(&addr.to_le_bytes());
            sh[16..20].copy_from_slice(&offset.to_le_bytes());
            sh[20..24].copy_from_slice(&size.to_le_bytes());
            sh[24..28].copy_from_slice(&link.to_le_bytes());
            f.extend(sh);
        }
        f.extend(image);
        f.extend(shstrtab);
        f.extend(strtab);
        f.extend(symtab);
        f
    }

    #[test]
    fn lays_segments_out_by_load_address() {
        let elf = Elf::parse(&sample()).unwrap();
        assert_eq!(
            elf.flash_image(),
            [0x0C, 0x94, 0x1F, 0x00, 0xFF, 0xFF, 0xAA, 0xBB]
        );
        let rodata = &elf.segments()[1];
        assert_eq!((rodata.vaddr, rodata.paddr), (0x80_8006, 6));
    }

    #[test]
    fn counts_allocated_sram_sections() {
        assert_eq!(Elf::parse(&sample()).unwrap().ram_bytes(), 12);
    }

    #[test]
    fn does_not_count_an_allocated_section_outside_sram() {
        for addr in [0, RAM.start - 1, RAM.end, DATA_SPACE + 0x8000] {
            let mut f = sample();
            let bss = 52 + 2 * 32;
            f[bss + 12..bss + 16].copy_from_slice(&addr.to_le_bytes());
            assert_eq!(Elf::parse(&f).unwrap().ram_bytes(), 0, "{addr:#x}");
        }
    }

    #[test]
    fn sram_is_the_same_range_in_elf_and_data_space_addresses() {
        assert_eq!(RAM, 0x80_3800..0x80_4000);
        assert_eq!(SRAM, 0x3800..0x4000);
    }

    #[test]
    fn takes_only_loadable_bytes_bound_for_flash() {
        const PT_NOTE: u32 = 4;
        let extra = [
            (PT_NOTE, 0x20, 0x20, 2),
            (PT_LOAD, 0x40, 0x40, 0),
            (PT_LOAD, DATA_SPACE, DATA_SPACE, 2),
            (PT_LOAD, RAM.start, RAM.start, 2),
        ];
        let elf = Elf::parse(&sample_with(&extra)).unwrap();
        assert_eq!(elf.segments().len(), 2);
        assert_eq!(
            elf.flash_image(),
            [0x0C, 0x94, 0x1F, 0x00, 0xFF, 0xFF, 0xAA, 0xBB]
        );

        let elf = Elf::parse(&sample_with(&[(PT_LOAD, 0, DATA_SPACE - 1, 1)])).unwrap();
        assert_eq!(elf.segments().len(), 3);
        assert_eq!(elf.segments()[2].paddr, DATA_SPACE - 1);
        assert_eq!(elf.segments()[2].data, [0x0C]);
    }

    #[test]
    fn lists_every_symbol_in_table_order() {
        let elf = Elf::parse(&sample()).unwrap();
        let names: Vec<&str> = elf.symbols().collect();
        assert_eq!(names, ["__reset", "main", "halt"]);
        assert_eq!(elf.symbol("halt"), Some(0xC4));
    }

    #[test]
    fn refuses_a_64_bit_or_a_big_endian_file() {
        for (byte, what) in [(4, "64-bit"), (5, "big-endian")] {
            let mut f = sample();
            f[byte] = 2;
            let Err(err) = Elf::parse(&f) else {
                panic!("parsed a {what} file");
            };
            assert!(err.contains("32-bit little-endian"), "{what}: {err}");
        }
    }

    #[test]
    fn refuses_a_file_that_was_cut_short() {
        let f = sample();
        // The last eight bytes are the end of the last symbol, which nothing
        // reads.
        for len in 4..f.len() - 8 {
            let Err(err) = Elf::parse(&f[..len]) else {
                panic!("parsed the first {len} bytes");
            };
            assert!(
                err.contains("truncated") || err.contains("unterminated"),
                "{len} bytes: {err}"
            );
        }
        assert!(Elf::parse(&f[..f.len() - 8]).is_ok());
    }

    #[test]
    fn reads_section_names_symbols_and_the_entry_point() {
        let elf = Elf::parse(&sample()).unwrap();
        assert_eq!(elf.section(".bss"), Some(0x80_3800..0x80_380C));
        assert_eq!(elf.section(".text"), None);
        assert_eq!(elf.symbol("__reset"), Some(0x7E));
        assert_eq!(elf.symbol("main"), Some(0xB2));
        assert_eq!(elf.symbol(""), None, "the null symbol isn't listed");
        assert_eq!(elf.entry(), 0x7E);
    }

    #[test]
    fn rejects_other_files() {
        assert!(Elf::parse(b"#!/bin/sh\n").is_err());
        let mut arm = sample();
        arm[18] = 40;
        assert!(Elf::parse(&arm).is_err());
    }
}
