//! `build.rs` loads this file by path, so nothing outside the tests may name
//! `crate::` or `ts26::`.

use std::collections::BTreeMap;
use std::fmt::Write;

struct Element {
    name: String,
    attrs: Vec<(String, String)>,
    children: Vec<Self>,
}

impl Element {
    fn attr(&self, key: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }

    fn get(&self, key: &str) -> Result<&str, String> {
        self.attr(key)
            .ok_or_else(|| format!("<{}> has no {key} attribute", self.name))
    }

    fn all<'a>(&'a self, tag: &'static str) -> impl Iterator<Item = &'a Self> {
        self.children.iter().filter(move |c| c.name == tag)
    }

    fn child(&self, tag: &str) -> Result<&Self, String> {
        self.children
            .iter()
            .find(|c| c.name == tag)
            .ok_or_else(|| format!("<{}> has no <{tag}>", self.name))
    }

    fn named(&self, tag: &str, name: &str) -> Result<&Self, String> {
        self.children
            .iter()
            .find(|c| c.name == tag && c.attr("name") == Some(name))
            .ok_or_else(|| format!("<{}> has no <{tag} name=\"{name}\">", self.name))
    }

    fn num(&self, key: &str) -> Result<u32, String> {
        num(self.get(key)?)
    }
}

fn num(s: &str) -> Result<u32, String> {
    let (digits, radix) = s
        .strip_prefix("0x")
        .or_else(|| s.strip_prefix("0X"))
        .map_or((s, 10), |hex| (hex, 16));
    u32::from_str_radix(digits, radix).map_err(|_| format!("{s:?} isn't a number"))
}

fn parse(xml: &str) -> Result<Element, String> {
    let mut stack = vec![Element {
        name: "#document".into(),
        attrs: Vec::new(),
        children: Vec::new(),
    }];
    let mut rest = xml;
    while let Some(i) = rest.find('<') {
        rest = &rest[i..];
        if let Some(r) = rest.strip_prefix("<!--") {
            let (_, after) = r.split_once("-->").ok_or("unterminated comment")?;
            rest = after;
        } else if rest.starts_with("<?") || rest.starts_with("<!") {
            let (_, after) = rest.split_once('>').ok_or("unterminated declaration")?;
            rest = after;
        } else if let Some(r) = rest.strip_prefix("</") {
            let (name, after) = r.split_once('>').ok_or("unterminated end tag")?;
            let element = stack.pop().ok_or("end tag with nothing open")?;
            if element.name != name.trim() {
                return Err(format!(
                    "mismatched end tag: <{}> closed by </{}>",
                    element.name,
                    name.trim()
                ));
            }
            stack
                .last_mut()
                .ok_or("end tag closes the document")?
                .children
                .push(element);
            rest = after;
        } else {
            let mut quote = None;
            let mut chars = rest.char_indices();
            let end = chars
                .find(|&(_, c)| match quote {
                    Some(q) if c == q => {
                        quote = None;
                        false
                    }
                    Some(_) => false,
                    None if c == '"' || c == '\'' => {
                        quote = Some(c);
                        false
                    }
                    None => c == '>',
                })
                .map(|(j, _)| j)
                .ok_or("unterminated start tag")?;
            let tag = &rest[1..end];
            rest = chars.as_str();
            let (tag, empty) = tag.strip_suffix('/').map_or((tag, false), |t| (t, true));
            let element = parse_tag(tag)?;
            if empty {
                stack
                    .last_mut()
                    .ok_or("no open element")?
                    .children
                    .push(element);
            } else {
                stack.push(element);
            }
        }
    }
    match <[Element; 1]>::try_from(stack) {
        Ok([document]) => Ok(document),
        Err(stack) => Err(format!(
            "unclosed <{}>",
            stack.last().map_or("", |e| &e.name)
        )),
    }
}

fn parse_tag(tag: &str) -> Result<Element, String> {
    let tag = tag.trim();
    let name_end = tag.find(char::is_whitespace).unwrap_or(tag.len());
    let mut element = Element {
        name: tag[..name_end].into(),
        attrs: Vec::new(),
        children: Vec::new(),
    };
    let mut rest = tag[name_end..].trim_start();
    while !rest.is_empty() {
        let (key, after) = rest
            .split_once('=')
            .ok_or_else(|| format!("bad attribute in <{tag}>"))?;
        let mut after = after.trim_start().chars();
        let q = after.next().filter(|&c| c == '"' || c == '\'');
        let q = q.ok_or_else(|| format!("unquoted attribute in <{tag}>"))?;
        let (value, after) = after
            .as_str()
            .split_once(q)
            .ok_or_else(|| format!("unterminated attribute in <{tag}>"))?;
        element.attrs.push((key.trim().into(), unescape(value)));
        rest = after.trim_start();
    }
    Ok(element)
}

fn unescape(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

struct Register<'a> {
    instance: &'a str,
    name: &'a str,
    address: u32,
    size: u32,
    element: &'a Element,
}

/// TCA0's come twice where its SINGLE and SPLIT modes share a name.
fn registers(document: &Element) -> Result<Vec<Register<'_>>, String> {
    let file = document.child("avr-tools-device-file")?;
    let device = file.child("devices")?.child("device")?;
    let mut registers = Vec::new();
    for module in device.child("peripherals")?.all("module") {
        let definition = file
            .child("modules")?
            .named("module", module.get("name")?)?;
        for instance in module.all("instance") {
            for group in instance.all("register-group") {
                let name = group.get("name-in-module")?;
                for element in definition.named("register-group", name)?.all("register") {
                    let (instance, name) = (instance.get("name")?, element.get("name")?);
                    registers.push(Register {
                        instance,
                        name,
                        address: group
                            .num("offset")?
                            .checked_add(element.num("offset")?)
                            .ok_or_else(|| format!("{instance}.{name}'s address overflows"))?,
                        size: element.num("size")?,
                        element,
                    });
                }
            }
        }
    }
    Ok(registers)
}

#[derive(Default)]
struct Consts(BTreeMap<String, (u32, String)>);

impl Consts {
    /// A name can turn up more than once (RTC.PITINTFLAGS.PI and
    /// RTC.PITINTCTRL.PI), which is fine while the value agrees.
    fn add(&mut self, mut name: String, value: u32, doc: String) {
        name.make_ascii_uppercase();
        if let Some((theirs, first)) = self.0.get(&name) {
            assert_eq!(
                *theirs, value,
                "{name} is both `{first}` and `{doc}`, with different values"
            );
        } else {
            self.0.insert(name, (value, doc));
        }
    }

    fn render(&self, kind: fn(u32) -> &'static str) -> String {
        self.0
            .iter()
            .fold(String::new(), |mut out, (name, (value, doc))| {
                let (kind, value) = (kind(*value), hex(*value));
                let _ = writeln!(out, "/// {doc}\npub const {name}: {kind} = {value};");
                out
            })
    }
}

// Six digits and up are grouped, as clippy::unreadable_literal asks.
fn hex(value: u32) -> String {
    if value > 0xF_FFFF {
        format!("0x{:X}_{:04X}", value >> 16, value & 0xFFFF)
    } else {
        format!("{value:#04X}")
    }
}

fn doc(path: String, element: &Element) -> String {
    match element.attr("caption") {
        Some(caption) if !caption.is_empty() => format!("{path}: {caption}"),
        _ => path,
    }
}

/// The `initval`s stay out, because they disagree with the datasheet on
/// ADC0.CALIB and the factory OSCCFG and SYSCFG0.
pub fn generate(xml: &str) -> Result<String, String> {
    let document = parse(xml)?;
    let file = document.child("avr-tools-device-file")?;
    let device = file.child("devices")?.child("device")?;
    let addr = addresses(&document)?;
    let mem = memory(device)?;
    let bits = bitfields(file)?;
    let vectors = vectors(device)?;
    let signature = signature(device)?;

    Ok(format!(
        "/// Data-space addresses of every register.\n\
         pub mod addr {{\n{}}}\n\n\
         /// Every memory segment. All of them are in data space but PROGMEM.\n\
         pub mod mem {{\n{}}}\n\n\
         {}\n{}\n\
         /// The device signature.\n\
         pub const SIGNATURE: [u8; 3] = {signature:#04X?};\n",
        addr.render(|_| "usize"),
        mem.render(|_| "usize"),
        bits.render(|value| if value > 0xFF { "u16" } else { "u8" }),
        vectors.render(|_| "usize"),
    ))
}

fn addresses(document: &Element) -> Result<Consts, String> {
    let mut addr = Consts::default();
    for r in registers(document)? {
        let width = match r.size {
            1 => String::new(),
            size => {
                let bits = size
                    .checked_mul(8)
                    .ok_or_else(|| format!("{}.{} is too big", r.instance, r.name))?;
                format!(", {bits}-bit")
            }
        };
        addr.add(
            format!("{}_{}", r.instance, r.name),
            r.address,
            doc(format!("{}.{}{width}", r.instance, r.name), r.element),
        );
    }
    Ok(addr)
}

fn memory(device: &Element) -> Result<Consts, String> {
    let mut mem = Consts::default();
    let spaces = device.child("address-spaces")?.all("address-space");
    for segment in spaces.flat_map(|space| space.all("memory-segment")) {
        let name = segment.get("name")?;
        let (start, size) = (segment.num("start")?, segment.num("size")?);
        mem.add(format!("{name}_START"), start, format!("{name}'s start."));
        mem.add(format!("{name}_SIZE"), size, format!("{name}'s size."));
        let end = start
            .checked_add(size)
            .ok_or_else(|| format!("{name} is too big"))?;
        let doc = "The first address after it.".to_string();
        mem.add(format!("{name}_END"), end, doc);
        if let Some(page) = segment.attr("pagesize") {
            let doc = format!("{name}'s page size.");
            mem.add(format!("{name}_PAGE"), num(page)?, doc);
        }
    }
    Ok(mem)
}

fn bitfields(file: &Element) -> Result<Consts, String> {
    let mut bits = Consts::default();
    for module in file.child("modules")?.all("module") {
        let prefix = module.get("name")?;
        let registers = module
            .all("register-group")
            .flat_map(|group| group.all("register"));
        for register in registers {
            for field in register.all("bitfield") {
                let mask = field.num("mask")?;
                let name = format!("{prefix}_{}", field.get("name")?);
                let path = format!("{prefix}.{}.{}", register.get("name")?, field.get("name")?);
                let values = match field.attr("values") {
                    Some(group) => Some(module.named("value-group", group)?.all("value")),
                    None => None,
                };
                for value in values.into_iter().flatten() {
                    let label = value.get("name")?;
                    let shifted = value.num("value")? << mask.trailing_zeros();
                    assert_eq!(shifted & !mask, 0, "{path}={label} spills out of its mask");
                    bits.add(
                        format!("{name}_{label}"),
                        shifted,
                        doc(format!("{path} = {label}"), value),
                    );
                }
                bits.add(name, mask, doc(path, field));
            }
        }
    }
    Ok(bits)
}

fn vectors(device: &Element) -> Result<Consts, String> {
    let mut vectors = Consts::default();
    let mut count = 0;
    for interrupt in device.child("interrupts")?.all("interrupt") {
        let index = interrupt.num("index")?;
        let (instance, name) = (interrupt.get("module-instance")?, interrupt.get("name")?);
        let after = index
            .checked_add(1)
            .ok_or_else(|| format!("{instance}.{name}'s vector is too high"))?;
        count = count.max(after);
        let path = format!("{instance}.{name}'s vector");
        vectors.add(
            format!("VECTOR_{instance}_{name}"),
            index,
            doc(path, interrupt),
        );
    }
    let doc = "Entries in the vector table, reset included.".to_string();
    vectors.add("VECTORS".into(), count, doc);
    Ok(vectors)
}

fn signature(device: &Element) -> Result<[u32; 3], String> {
    let property = |name: &str| {
        let groups = device.child("property-groups")?.all("property-group");
        groups
            .flat_map(|group| group.all("property"))
            .find(|p| p.attr("name") == Some(name))
            .ok_or_else(|| format!("no property {name}"))?
            .num("value")
    };
    Ok([
        property("SIGNATURE0")?,
        property("SIGNATURE1")?,
        property("SIGNATURE2")?,
    ])
}

#[cfg(test)]
mod tests {
    use std::fs;

    use ts26::regs::{self, addr, mem};

    use super::*;

    fn atdf() -> String {
        let path = crate::root().join("vendor/ATtiny1616.atdf");
        fs::read_to_string(&path).unwrap_or_else(|e| panic!("can't read {}: {e}", path.display()))
    }

    #[test]
    fn parses_attributes_comments_and_nesting() {
        let doc = parse(
            "<?xml version=\"1.0\"?>\n<!-- a <comment> -->\n\
             <a x=\"1\" y='a &gt; b'><b name=\"c\"/><b name=\"d\">text</b></a>",
        )
        .unwrap();
        let a = doc.child("a").unwrap();
        assert_eq!(a.attr("x"), Some("1"));
        assert_eq!(a.attr("y"), Some("a > b"));
        assert_eq!(a.all("b").count(), 2);
        assert_eq!(a.named("b", "d").unwrap().name, "b");
        assert_eq!(num("0x0A00"), Ok(0x0A00));
        assert_eq!(num("16"), Ok(16));
    }

    /// From the datasheet, so a generator that goes wrong can't agree with
    /// itself.
    #[test]
    fn generates_each_kind_of_constant() {
        assert_eq!(addr::CPU_CCP, 0x0034, "a plain address");
        assert_eq!(addr::ADC0_CTRLA, 0x0600, "base plus offset");
        assert_eq!(addr::TCA0_CMP1BUF, 0x0A3A, "a SINGLE mode register");
        assert_eq!(addr::FUSE_BOOTEND, 0x1288, "a fuse");
        assert_eq!(mem::INTERNAL_SRAM_START, 0x3800, "a segment's start");
        assert_eq!(mem::INTERNAL_SRAM_SIZE, 0x0800, "a segment's size");
        assert_eq!(mem::INTERNAL_SRAM_END, 0x4000, "a segment's end");
        assert_eq!(mem::EEPROM_PAGE, 32, "a segment's page size");
        assert_eq!(regs::PORT_PULLUPEN, 0x08, "a mask");
        assert_eq!(regs::TCA_CMP1EN, 0x20, "a mask on a mode's register");
        assert_eq!(regs::TCD_CMPASET, 0x0FFF, "a mask wider than a byte");
        assert_eq!(regs::ADC_REFSEL_VDDREF, 0x10, "a shifted value");
        assert_eq!(regs::RTC_PERIOD_CYC512, 0x40, "a shifted value");
        assert_eq!(regs::ADC_REFSEL_INTREF, 0x00, "a value of zero");
        assert_eq!(regs::VECTOR_RTC_PIT, 7, "a vector");
        assert_eq!(regs::VECTORS, 31, "the vector count");
        assert_eq!(regs::SIGNATURE, [0x1E, 0x94, 0x21], "the signature");
    }

    #[test]
    fn generated_constants_say_where_they_came_from() {
        let source = generate(&atdf()).unwrap();
        for line in [
            "pub const ADC0_RES: usize = 0x610;",
            "pub const TCD_CMPASET: u16 = 0xFFF;",
            "pub const RTC_PI: u8 = 0x01;",
        ] {
            assert!(source.contains(line), "{line}");
        }
        assert!(source.contains("/// ADC0.RES, 16-bit: "), "the width");
    }

    #[test]
    fn repeated_names_are_fine_when_they_agree() {
        let mut consts = Consts::default();
        consts.add("A".into(), 1, "first".into());
        consts.add("A".into(), 1, "second".into());
        assert_eq!(
            consts.render(|_| "u8"),
            "/// first\npub const A: u8 = 0x01;\n"
        );
    }

    #[test]
    fn six_hex_digits_and_up_are_grouped() {
        assert_eq!(hex(0xF_FFFF), "0xFFFFF");
        assert_eq!(hex(0x10_0000), "0x10_0000");
        assert_eq!(hex(0xFFFF_FFFF), "0xFFFF_FFFF");
    }

    #[test]
    #[should_panic(expected = "A is both `first` and `second`")]
    fn repeated_names_that_disagree_stop_the_build() {
        let mut consts = Consts::default();
        consts.add("A".into(), 1, "first".into());
        consts.add("A".into(), 2, "second".into());
    }

    #[test]
    fn the_16_bit_registers_are_16_bits() {
        let xml = atdf();
        let document = parse(&xml).unwrap();
        let registers = registers(&document).unwrap();
        for wide in [
            addr::TCA0_CNT,
            addr::TCA0_PER,
            addr::TCA0_CMP0,
            addr::TCA0_PERBUF,
            addr::TCA0_CMP0BUF,
            addr::TCA0_CMP2BUF,
            addr::ADC0_RES,
        ] {
            let found = registers
                .iter()
                .any(|r| r.address as usize == wide && r.size == 2);
            assert!(found, "nothing 16 bits wide at {wide:#06x}");
        }
    }

    #[test]
    fn pin_controls_are_consecutive() {
        let xml = atdf();
        let document = parse(&xml).unwrap();
        let registers = registers(&document).unwrap();
        for (base, port) in [
            (addr::PORTA_PIN0CTRL, "PORTA"),
            (addr::PORTB_PIN0CTRL, "PORTB"),
            (addr::PORTC_PIN0CTRL, "PORTC"),
        ] {
            for n in 0..8 {
                let name = format!("PIN{n}CTRL");
                let register = registers
                    .iter()
                    .find(|r| r.instance == port && r.name == name)
                    .unwrap_or_else(|| panic!("no {port}.{name}"));
                assert_eq!(base + n, register.address as usize, "{port}.{name}");
            }
        }
    }

    /// 0x800000 is a toolchain convention the ATDF doesn't record.
    #[test]
    fn linker_script_matches_the_atdf() {
        let data = crate::elf::DATA_SPACE;
        let mapped = mem::MAPPED_PROGMEM_START as u32;

        let script = link_x();
        let (memory, sections) = linker_script(&script);
        assert_eq!(
            memory,
            [
                (
                    "FLASH".into(),
                    mem::PROGMEM_START as u32,
                    mem::PROGMEM_SIZE as u32
                ),
                (
                    "MAPPED".into(),
                    data + mapped,
                    mem::MAPPED_PROGMEM_SIZE as u32
                ),
                (
                    "RAM".into(),
                    data + mem::INTERNAL_SRAM_START as u32,
                    mem::INTERNAL_SRAM_SIZE as u32
                ),
            ],
            "link.x MEMORY"
        );
        for n in sections {
            assert!(
                n == data + mapped || n == mapped,
                "link.x SECTIONS has {n:#x}, which isn't the mapped-flash window"
            );
        }
    }

    #[test]
    fn parses_a_linker_script() {
        let (memory, sections) = linker_script(
            "/* MEMORY { X : ORIGIN = 1 } 0x1 */\n\
             MEMORY {\n  A (rx) : ORIGIN = 0x10, LENGTH = 2K\n  B : ORIGIN = 0, LENGTH = 0x20\n}\n\
             SECTIONS { .a (0x800 + 1) : AT(0x9) { } }",
        );
        assert_eq!(memory, [("A".into(), 0x10, 2048), ("B".into(), 0, 0x20)]);
        assert_eq!(sections, [0x800, 0x9]);
    }

    fn link_x() -> String {
        let path = crate::root().join("link.x");
        fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
    }

    fn linker_script(script: &str) -> (Vec<(String, u32, u32)>, Vec<u32>) {
        let mut text = String::new();
        let mut rest = script;
        while let Some(at) = rest.find("/*") {
            text.push_str(&rest[..at]);
            let end = rest[at..].find("*/").expect("unterminated comment");
            rest = &rest[at + end + 2..];
        }
        text.push_str(rest);

        let start = text.find("MEMORY").expect("no MEMORY block");
        let open = start + text[start..].find('{').expect("no { after MEMORY");
        let close = open + text[open..].find('}').expect("no } after MEMORY");
        let memory = text[open + 1..close]
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(|line| {
                let (name, spec) = line.split_once(':').expect("region without :");
                let name = name.split_whitespace().next().expect("region name");
                let field = |key: &str| {
                    let value = spec
                        .split(',')
                        .find_map(|f| f.trim().strip_prefix(key))
                        .and_then(|f| f.trim().strip_prefix('='))
                        .unwrap_or_else(|| panic!("{name} has no {key}"))
                        .trim();
                    let (digits, scale) = value.strip_suffix('K').map_or((value, 1), |k| (k, 1024));
                    num(digits).unwrap() * scale
                };
                (name.to_string(), field("ORIGIN"), field("LENGTH"))
            })
            .collect();

        let numbers = text[close + 1..]
            .split(|c: char| !c.is_ascii_alphanumeric())
            .filter(|w| w.starts_with("0x"))
            .map(|w| num(w).unwrap())
            .collect();
        (memory, numbers)
    }
}
