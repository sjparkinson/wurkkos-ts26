//! `cargo xtask <command>` is the AVR build and the steps around it that cargo
//! can't do.

#[cfg(test)]
mod atdf;
mod avr;
mod breakages;
mod crc;
mod elf;
mod ihex;
mod layout;
#[cfg(test)]
mod lints;
mod serial;
mod sim;
mod simulate;
mod stamp;
mod switching;
mod terminal;
mod updi;
mod world;

use std::env;
use std::fmt::Display;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use serial::Port;
use updi::{Programmer, SerialNumber, Wire, map};

const USAGE: &str = "\
usage: cargo xtask <command>

commands:
  hex                     build the firmware, run it on the simulator, write
                          target/wurkkos-ts26-0721.hex and print flash, RAM,
                          stack, loop and handler use
  hex --no-examples       the same, leaving the UI examples out of the
                          simulator run, for a breakout test build (T1, T5)
  breakages [name...]     check hex refuses copies of the firmware, each broken on
                          purpose in one way, and leave the checkout alone
  ping <port>           check the programmer can see the chip, writing nothing,
                          though expect it to reset as UPDI turns off
  peek <port>             read the running chip's registers and SRAM, to see how
                          far the firmware got; it writes nothing, and the same
                          reset follows, so only the first run counts
  backup <port> <dir>     save the chip's flash, EEPROM, fuses and serial number
                          into <dir>
  flash <port> [file.hex] build and flash the firmware, or flash file.hex
  restore <port> <dir>    put <dir>/flash.hex and <dir>/eeprom.hex back, on the
                          chip whose serial number is in <dir>/serial.hex and
                          no other
  simulate [script...]    try src/ui.rs live in the terminal, or on a script
                          such as `1C 2H:1.5s wait:1s 3C`

<port> is the serial UPDI adapter, e.g. /dev/cu.usbserial-1410 on macOS
or /dev/ttyUSB0 on Linux. <dir> names one chip's backup, e.g. backup/torch.
";

const ELF: &str = "target/avr-none/release/ts26.elf";
const PROBE: &str = "target/avr-none/release/examples/link.elf";
const GUARDS: &str = "target/avr-none/release/examples/guards.elf";
const HEX: &str = "target/wurkkos-ts26-0721.hex";

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();

    let result = match args.as_slice() {
        ["hex"] => hex(true),
        ["hex", "--no-examples"] => hex(false),
        ["breakages", names @ ..] => breakages::run(&root(), names),
        ["ping", port] => ping(port),
        ["peek", port] => peek(port),
        ["backup", port, dir] => backup(port, Path::new(dir)),
        ["flash", port] => build(true)
            .and_then(|(elf, _)| crc::sign(&elf.flash_image(), &stamp::current(&root())))
            .and_then(|image| flash(port, &image)),
        ["flash", port, file] => this_boards_hex(Path::new(file))
            .and_then(|()| read_hex(Path::new(file)))
            .and_then(|image| signed(&image).map(|()| image))
            .and_then(|image| flash(port, &image)),
        ["restore", port, dir] => restore(port, Path::new(dir)),
        ["simulate"] => simulate::interactive(),
        ["simulate", script @ ..] => simulate::script(script),
        _ => {
            eprint!("{USAGE}");
            return ExitCode::from(2);
        }
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask has a parent directory")
        .to_path_buf()
}

fn build(examples: bool) -> Result<(elf::Elf, sim::Report), String> {
    let cargo = env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    run(Command::new(cargo).current_dir(root()).args([
        "build",
        "--package",
        "ts26",
        "--bins",
        "--example",
        "link",
        "--example",
        "guards",
        "--release",
        "--target",
        "avr-none",
        "-Zbuild-std=core",
    ]))?;
    let path = root().join(ELF);
    let bytes = fs::read(&path).map_err(at(&path))?;
    let elf = elf::Elf::parse(&bytes).map_err(at(&path))?;
    layout::check(&elf).map_err(at(&path))?;
    let report =
        sim::check(&elf, examples).map_err(|e| format!("{}: simulator: {e}", path.display()))?;

    let probe = root().join(PROBE);
    let bytes = fs::read(&probe).map_err(at(&probe))?;
    let probe_elf = elf::Elf::parse(&bytes).map_err(at(&probe))?;
    probe_sections(&probe_elf).map_err(at(&probe))?;
    layout::memory(&probe_elf).map_err(at(&probe))?;
    sim::probe(&probe_elf).map_err(|e| format!("{}: simulator: {e}", probe.display()))?;

    let guards = root().join(GUARDS);
    let bytes = fs::read(&guards).map_err(at(&guards))?;
    let guards_elf = elf::Elf::parse(&bytes).map_err(at(&guards))?;
    layout::check(&guards_elf).map_err(at(&guards))?;
    sim::guards(&guards_elf).map_err(|e| format!("{}: simulator: {e}", guards.display()))?;
    Ok((elf, report))
}

/// `layout::memory` passes an empty .data without looking.
fn probe_sections(elf: &elf::Elf) -> Result<(), String> {
    for name in [".rodata", ".data", ".bss"] {
        match elf.section(name) {
            Some(range) if !range.is_empty() => {}
            _ => return Err(format!("the link probe has no {name}")),
        }
    }
    Ok(())
}

fn hex(examples: bool) -> Result<(), String> {
    let (elf, sim) = build(examples)?;
    let firmware = elf.flash_image();
    let image = crc::sign(&firmware, &stamp::current(&root()))?;
    write(&root().join(HEX), &ihex::encode(&image))?;
    eprintln!("wrote {HEX}");
    println!("stamp {}", stamp::read(&image).unwrap_or_default());

    let flash = firmware.len() as u64;
    let ram = elf.ram_bytes().into();
    let ram_size = (elf::RAM.end - elf::RAM.start).into();
    println!(
        "{}",
        row("flash", flash, Some(map::FLASH_SIZE as u64), "bytes")
    );
    println!("{}", row("ram", ram, Some(ram_size), "bytes"));
    println!("{}", row("stack", sim.stack.into(), None, "bytes"));
    println!(
        "{}",
        row(
            "loop",
            sim.loop_instructions,
            Some(sim.loop_budget),
            "instructions per 64 Hz tick"
        )
    );
    println!(
        "{}",
        row(
            "off",
            sim.off_instructions,
            Some(sim.loop_budget),
            "instructions per 64 Hz tick"
        )
    );
    println!(
        "{}",
        row(
            "dsm",
            sim.handler_instructions,
            Some(sim.handler_budget),
            "instructions per PWM cycle"
        )
    );
    Ok(())
}

fn row(label: &str, used: u64, of: Option<u64>, unit: &str) -> String {
    let (of, percent) = match of {
        Some(of) => (
            format!("/ {:>6}", commas(of)),
            format!("{:>3}%", used * 100 / of),
        ),
        None => (String::new(), String::new()),
    };
    format!(
        "{label:<5} {:>6} {of:<8} {percent:>4}  {unit}",
        commas(used)
    )
}

fn commas(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, digit) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

/// Anything that can refuse without a chip does it before this.
fn open(port: &str) -> Result<Port, String> {
    eprintln!("connecting on {port}");
    Port::open(port)
}

fn connect(port: &str) -> Result<Programmer<Port>, String> {
    Programmer::connect(open(port)?)
}

fn ping(port: &str) -> Result<(), String> {
    let mut p = connect(port)?;
    let sib = p.sib()?;
    p.disconnect()?;
    println!("found {}", String::from_utf8_lossy(&sib).trim_end());
    Ok(())
}

/// Turning UPDI off afterwards should reset the chip (DS §33.3.2.2.2), so a
/// second read sees a fresh boot.
fn peek(port: &str) -> Result<(), String> {
    use ts26::regs::addr;
    let registers = [
        ("RSTCTRL.RSTFR", addr::RSTCTRL_RSTFR),
        ("CLKCTRL.MCLKCTRLB", addr::CLKCTRL_MCLKCTRLB),
        ("WDT.CTRLA", addr::WDT_CTRLA),
        ("CPU.SPL", usize::from(avr::SPL)),
        ("CPU.SPH", usize::from(avr::SPH)),
        ("CPU.SREG", addr::CPU_SREG),
        ("VPORTB.DIR", addr::VPORTB_DIR),
        ("VPORTB.OUT", addr::VPORTB_OUT),
        ("VPORTC.DIR", addr::VPORTC_DIR),
        ("TCA0.CTRLA", addr::TCA0_CTRLA),
        ("TCA0.CTRLB", addr::TCA0_CTRLB),
        ("RTC.PITCTRLA", addr::RTC_PITCTRLA),
    ];

    let mut p = connect(port)?;
    for (name, at) in registers {
        println!("{name:18} {:02X}", p.read(at as u16, 1)?[0]);
    }
    let sram = p.read(
        elf::SRAM.start,
        usize::from(elf::SRAM.end - elf::SRAM.start),
    )?;
    p.disconnect()?;

    println!("SRAM");
    for line in runs(elf::SRAM.start, &sram) {
        println!("  {line}");
    }
    Ok(())
}

fn runs(start: u16, bytes: &[u8]) -> Vec<String> {
    bytes
        .chunk_by(|a, b| a == b)
        .scan(start, |at, run| {
            let line = format!("{at:04X} {:02X} x{}", run[0], run.len());
            *at += run.len() as u16;
            Some(line)
        })
        .collect()
}

/// An image has no model in it, so the name's all there is.
fn this_boards_hex(path: &Path) -> Result<(), String> {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    if name.contains("ts26") || name.contains("0721") {
        return Ok(());
    }
    Err(format!(
        "{} doesn't say it's for the TS26 (model 0721), so it won't be flashed. Flash the \
         file `cargo xtask hex` writes, or use `restore <port> <dir>` for a backup",
        path.display()
    ))
}

/// An image with a bad CRC flashes and verifies cleanly, then caps itself at
/// level 50 with nothing to say why. A short one (the stock firmware, T0)
/// needs no CRC.
fn signed(image: &[u8]) -> Result<(), String> {
    if image.len() < map::FLASH_SIZE {
        eprintln!(
            "warning: the image isn't the full {} bytes with a CRC, so it isn't a signed build \
             of the full firmware. That's right for the stock firmware and for T0 (first light), which check \
             no CRC. If it's a build from main, it'll cap itself at level 50 on every boot, so \
             flash the file `cargo xtask hex` writes instead",
            map::FLASH_SIZE
        );
        return Ok(());
    }
    if crc::crc16(image.iter().copied()) != 0 {
        return Err(
            "the image's CRC doesn't check, so the firmware would cap itself at level 50 on \
             every boot. Flash the file `cargo xtask hex` writes"
                .to_string(),
        );
    }
    Ok(())
}

fn backup(port: &str, dir: &Path) -> Result<(), String> {
    backup_from(|| open(port), dir)
}

fn backup_from<W: Wire>(
    open: impl FnOnce() -> Result<W, String>,
    dir: &Path,
) -> Result<(), String> {
    let flash_path = dir.join("flash.hex");
    if flash_path.exists() {
        return Err(format!(
            "{} already exists. Move it somewhere safe first; if it's the factory \
             firmware, it's the only copy",
            flash_path.display()
        ));
    }

    let mut p = Programmer::connect_for_programming(open()?)?;
    let read = read_chip(&mut p);
    // Even after a failed read, or the chip's held until a power cycle.
    let left = p.leave();
    let chip = read?;
    report_stamp("the chip's flash", &chip.flash);

    fs::create_dir_all(dir).map_err(at(dir))?;
    write(&flash_path, &ihex::encode(&chip.flash))?;
    write(&dir.join("eeprom.hex"), &ihex::encode(&chip.eeprom))?;
    write(&dir.join("fuses.hex"), &ihex::encode(&chip.fuses))?;
    write(&dir.join("serial.hex"), &ihex::encode(chip.serial.bytes()))?;
    eprintln!(
        "saved flash, EEPROM, fuses (for reference, never written back) and the chip's serial \
         number, {}, to {}",
        chip.serial,
        dir.display()
    );
    // Not fatal, since this may be the only copy of the factory firmware, but
    // `restore` will refuse it, so say so before anything's flashed over it.
    if chip.serial.is_blank() {
        eprintln!(
            "warning: that serial number can't tell one chip from another. The backup is \
             saved all the same, but `restore` will refuse it, since it couldn't check it \
             was writing to the chip this came from"
        );
    }
    // Saved whatever leaving said, since it may be the only factory copy.
    left?;
    Ok(())
}

struct Chip {
    flash: Vec<u8>,
    eeprom: Vec<u8>,
    /// The fuses, then the lock byte.
    fuses: Vec<u8>,
    serial: SerialNumber,
}

fn read_chip<W: Wire>(p: &mut Programmer<W>) -> Result<Chip, String> {
    eprintln!("reading flash");
    let flash = p.read(map::FLASH, map::FLASH_SIZE)?;
    eprintln!("reading EEPROM, fuses and the serial number");
    let eeprom = p.read(map::EEPROM, map::EEPROM_SIZE)?;
    let fuses = p.read(map::FUSES, map::FUSES_SIZE)?;
    let serial = p.serial_number()?;
    Ok(Chip {
        flash,
        eeprom,
        fuses,
        serial,
    })
}

fn report_stamp(what: &str, flash: &[u8]) {
    match stamp::read(flash) {
        Some(text) => eprintln!("{what} has the stamp `{text}`"),
        None => eprintln!(
            "{what} has no stamp, so it isn't a full build from this repo (it may be the \
             stock firmware, T0 (first light) or an empty chip)"
        ),
    }
}

fn flash(port: &str, image: &[u8]) -> Result<(), String> {
    report_stamp("the image", image);
    let connect = || Programmer::connect_for_programming(open(port)?);
    program(connect, image, None)
}

fn restore(port: &str, dir: &Path) -> Result<(), String> {
    restore_onto(|| open(port), dir)
}

fn restore_onto<W: Wire>(
    open: impl FnOnce() -> Result<W, String>,
    dir: &Path,
) -> Result<(), String> {
    let backup = read_backup(dir)?;
    let connect = || {
        let p = Programmer::connect_for_restore(open()?, &backup.serial, dir)?;
        eprintln!(
            "this chip's serial number, {}, is the one saved in {}",
            backup.serial,
            dir.display()
        );
        Ok(p)
    };
    program(connect, &backup.flash, Some(&backup.eeprom))
}

struct Backup {
    flash: Vec<u8>,
    eeprom: Vec<u8>,
    /// The only chip `restore` writes these to.
    serial: SerialNumber,
}

/// Fuses aren't read, because nothing writes them back.
fn read_backup(dir: &Path) -> Result<Backup, String> {
    let serial_path = dir.join("serial.hex");
    if !serial_path.try_exists().map_err(at(&serial_path))? {
        return Err(format!(
            "{} doesn't exist, so nothing says which chip this backup was read from. \
             `restore` only writes a backup onto the chip it was read from, and the serial \
             number in that file is how it tells, so it won't write this one",
            serial_path.display()
        ));
    }
    let serial = SerialNumber::from_bytes(&read_hex(&serial_path)?).map_err(at(&serial_path))?;
    serial.check_saved(dir)?;
    Ok(Backup {
        flash: read_hex(&dir.join("flash.hex"))?,
        eeprom: read_hex(&dir.join("eeprom.hex"))?,
        serial,
    })
}

/// The chip erase wipes EEPROM too unless the EESAVE fuse is set.
fn program<W: Wire>(
    connect: impl FnOnce() -> Result<Programmer<W>, String>,
    flash: &[u8],
    eeprom: Option<&[u8]>,
) -> Result<(), String> {
    if flash.len() > map::FLASH_SIZE {
        return Err(format!(
            "the image is {} bytes; the ATtiny1616 has {}",
            flash.len(),
            map::FLASH_SIZE
        ));
    }
    if eeprom.is_some_and(|e| e.len() > map::EEPROM_SIZE) {
        return Err(format!(
            "the EEPROM image is bigger than {} bytes",
            map::EEPROM_SIZE
        ));
    }

    let mut p = connect()?;
    let written = write_and_verify(&mut p, flash, eeprom);
    // Even after a failure, or the chip's held until a power cycle.
    let left = p.leave();
    written?;
    left?;
    eprintln!("verified and restarted");
    Ok(())
}

fn write_and_verify<W: Wire>(
    p: &mut Programmer<W>,
    flash: &[u8],
    eeprom: Option<&[u8]>,
) -> Result<(), String> {
    eprintln!("erasing and writing {} bytes of flash", flash.len());
    p.erase_and_write_flash(flash)?;
    p.verify(map::FLASH, flash)?;
    if let Some(eeprom) = eeprom {
        eprintln!("writing EEPROM");
        p.write(map::EEPROM, map::EEPROM_PAGE, eeprom)?;
        p.verify(map::EEPROM, eeprom)?;
    }
    Ok(())
}

fn read_hex(path: &Path) -> Result<Vec<u8>, String> {
    let text = fs::read_to_string(path).map_err(at(path))?;
    ihex::decode(&text).map_err(at(path))
}

fn write(path: &Path, contents: &str) -> Result<(), String> {
    fs::write(path, contents).map_err(at(path))
}

fn at<E: Display>(path: &Path) -> impl Fn(E) -> String + '_ {
    move |e| format!("{}: {e}", path.display())
}

fn run(command: &mut Command) -> Result<(), String> {
    let program = command.get_program().to_string_lossy().into_owned();
    let status = command
        .status()
        .map_err(|e| format!("couldn't run {program}: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{program} failed ({status})"))
    }
}

#[cfg(test)]
mod tests {
    use std::env;
    use std::fs;
    use std::path::{Path, PathBuf};

    use super::updi::tests::fake::Tiny;
    use super::updi::{Programmer, SerialNumber};
    use super::{
        backup_from, commas, crc, ihex, program, read_hex, restore_onto, row, runs, signed, stamp,
        this_boards_hex, write,
    };

    struct Scratch(PathBuf);

    impl Scratch {
        fn new(test: &str) -> Self {
            let dir = env::temp_dir().join(format!("ts26-xtask-{}-{test}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        fn backup(&self) -> PathBuf {
            self.0.join("backup").join("torch")
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn chip() -> Tiny {
        let mut tiny = Tiny::new();
        for (i, byte) in tiny.flash.iter_mut().take(1000).enumerate() {
            *byte = (i * 7 + 3) as u8;
        }
        for (i, byte) in tiny.eeprom.iter_mut().enumerate() {
            *byte = i as u8;
        }
        tiny
    }

    fn on<'a>(tiny: &'a mut Tiny) -> impl FnOnce() -> Result<&'a mut Tiny, String> {
        move || Ok(tiny)
    }

    fn no_port() -> Result<Tiny, String> {
        panic!("the port was opened");
    }

    fn shown(serial: &[u8]) -> String {
        SerialNumber::from_bytes(serial).unwrap().to_string()
    }

    #[test]
    fn backup_saves_the_serial_number_and_restore_puts_the_backup_back() {
        let scratch = Scratch::new("restores");
        let dir = scratch.backup();
        let mut tiny = chip();
        let (flash, eeprom) = (tiny.flash.clone(), tiny.eeprom.clone());

        backup_from(on(&mut tiny), &dir).unwrap();
        assert_eq!(read_hex(&dir.join("flash.hex")).unwrap(), flash);
        assert_eq!(read_hex(&dir.join("eeprom.hex")).unwrap(), eeprom);
        assert_eq!(read_hex(&dir.join("serial.hex")).unwrap(), tiny.serial);
        assert!(dir.join("fuses.hex").exists(), "no fuses.hex");

        tiny.flash.fill(0x42);
        tiny.eeprom.fill(0x24);
        restore_onto(on(&mut tiny), &dir).unwrap();
        assert!(tiny.flash == flash, "flash isn't the backup's");
        assert!(tiny.eeprom == eeprom, "EEPROM isn't the backup's");
        assert!(tiny.updi_disabled, "UPDI was left on");
        assert!(!tiny.progmode, "the chip was left in programming mode");
    }

    #[test]
    fn restore_refuses_a_backup_from_another_chip_and_writes_nothing() {
        let scratch = Scratch::new("another-chip");
        let dir = scratch.backup();
        let mut breakout = chip();
        backup_from(on(&mut breakout), &dir).unwrap();

        let mut torch = Tiny::new();
        torch.serial[4] ^= 0x80;
        torch.flash.fill(0x42);
        torch.eeprom.fill(0x24);
        let err = restore_onto(on(&mut torch), &dir).unwrap_err();

        assert!(err.contains(&shown(&torch.serial)), "{err}");
        assert!(err.contains(&shown(&breakout.serial)), "{err}");
        assert!(err.contains(&dir.display().to_string()), "{err}");
        assert!(torch.flash.iter().all(|&b| b == 0x42), "flash changed");
        assert!(torch.eeprom.iter().all(|&b| b == 0x24), "EEPROM changed");
        assert_eq!(torch.page_writes + torch.page_erase_writes, 0);
        assert!(torch.updi_disabled, "UPDI was left on");
        assert!(!torch.progmode, "the chip was left in programming mode");
        assert!(!torch.in_reset, "the chip was left in reset");
    }

    #[test]
    fn restore_refuses_a_backup_with_no_serial_number_before_it_opens_the_port() {
        let scratch = Scratch::new("no-serial");
        let dir = scratch.backup();
        backup_from(on(&mut chip()), &dir).unwrap();
        fs::remove_file(dir.join("serial.hex")).unwrap();

        let err = restore_onto(no_port, &dir).unwrap_err();
        assert!(err.contains("serial.hex doesn't exist"), "{err}");
        assert!(err.contains("which chip"), "{err}");
    }

    #[test]
    fn restore_refuses_a_serial_number_of_the_wrong_size_before_it_opens_the_port() {
        let scratch = Scratch::new("short-serial");
        let dir = scratch.backup();
        backup_from(on(&mut chip()), &dir).unwrap();
        write(&dir.join("serial.hex"), &ihex::encode(&[0x12; 9])).unwrap();

        let err = restore_onto(no_port, &dir).unwrap_err();
        assert!(err.contains("serial.hex"), "{err}");
        assert!(err.contains("10 bytes, not 9"), "{err}");
    }

    #[test]
    fn backup_saves_a_blank_serial_number_and_restore_refuses_it_before_it_opens_the_port() {
        for blank in [0xFF, 0x00] {
            let scratch = Scratch::new(&format!("blank-{blank:02x}"));
            let dir = scratch.backup();
            let mut tiny = chip();
            tiny.serial.fill(blank);

            backup_from(on(&mut tiny), &dir).unwrap();
            assert_eq!(read_hex(&dir.join("flash.hex")).unwrap(), tiny.flash);
            assert_eq!(read_hex(&dir.join("serial.hex")).unwrap(), tiny.serial);

            let err = restore_onto(no_port, &dir).unwrap_err();
            assert!(err.contains(&format!("all {blank:02X}")), "{err}");
            assert!(err.contains(&dir.display().to_string()), "{err}");
        }
    }

    #[test]
    fn backup_refuses_to_overwrite_one_before_it_opens_the_port() {
        let scratch = Scratch::new("overwrite");
        let dir = scratch.backup();
        backup_from(on(&mut chip()), &dir).unwrap();

        let err = backup_from(no_port, &dir).unwrap_err();
        assert!(err.contains("already exists"), "{err}");
    }

    #[test]
    fn flash_takes_only_a_signed_image() {
        let image = crc::sign(&[0x12, 0x34, 0x56], &stamp::TEST).unwrap();
        let round_trip = ihex::decode(&ihex::encode(&image)).unwrap();
        assert!(signed(&round_trip).is_ok());

        let mut broken = image;
        broken[1] ^= 1;
        assert!(signed(&broken).is_err());
    }

    #[test]
    fn a_stamp_survives_a_flash_and_a_backup() {
        let scratch = Scratch::new("stamp");
        let dir = scratch.backup();
        let image = crc::sign(&[0x12, 0x34, 0x56], &stamp::format("abc1234567", true)).unwrap();
        let mut tiny = Tiny::new();

        let open = on(&mut tiny);
        program(
            || Programmer::connect_for_programming(open()?),
            &image,
            None,
        )
        .unwrap();
        assert!(tiny.flash == image, "flash isn't the image");

        backup_from(on(&mut tiny), &dir).unwrap();
        let saved = read_hex(&dir.join("flash.hex")).unwrap();
        assert_eq!(
            stamp::read(&saved).as_deref(),
            Some("ts26 abc1234567+"),
            "the backup lost the stamp"
        );
    }

    #[test]
    fn flash_still_takes_a_short_image_such_as_the_stock_firmware() {
        assert!(signed(&[0x12, 0x34, 0x56]).is_ok());
    }

    #[test]
    fn runs_show_where_a_pattern_stops() {
        assert_eq!(
            runs(0x3800, &[0xAA, 0xAA, 0xAA, 0x55, 0x00, 0x00]),
            ["3800 AA x3", "3803 55 x1", "3804 00 x2"]
        );
    }

    #[test]
    fn flash_takes_this_boards_hex_and_refuses_another_torchs() {
        for good in [
            "target/wurkkos-ts26-0721.hex",
            "release.wurkkos-ts26.hex",
            "WURKKOS-TS26.HEX",
        ] {
            assert!(this_boards_hex(Path::new(good)).is_ok(), "{good}");
        }
        for bad in [
            "target/wurkkos-ts10.hex",
            "backup/torch/flash.hex",
            "ts26/firmware.hex",
        ] {
            let err = this_boards_hex(Path::new(bad)).unwrap_err();
            assert!(err.contains("model 0721"), "{bad}: {err}");
            assert!(err.contains("`restore <port> <dir>`"), "{bad}: {err}");
        }
    }

    #[test]
    fn rows_line_up_with_and_without_a_total() {
        assert_eq!(
            row("flash", 3978, Some(16384), "bytes"),
            "flash  3,978 / 16,384  24%  bytes"
        );
        assert_eq!(
            row("stack", 107, None, "bytes"),
            "stack    107                bytes"
        );
    }

    #[test]
    fn commas_group_thousands() {
        assert_eq!(commas(0), "0");
        assert_eq!(commas(107), "107");
        assert_eq!(commas(3978), "3,978");
        assert_eq!(commas(39062), "39,062");
        assert_eq!(commas(1_234_567), "1,234,567");
    }

    const FLASH_SIZE: usize = super::updi::map::FLASH_SIZE;
    const EEPROM_SIZE: usize = super::updi::map::EEPROM_SIZE;

    fn no_connection() -> Result<Programmer<Tiny>, String> {
        Programmer::connect_for_programming(no_port()?)
    }

    #[test]
    fn an_image_bigger_than_flash_is_refused_before_the_port_is_opened() {
        let err = program(no_connection, &vec![0x12; FLASH_SIZE + 1], None).unwrap_err();
        assert_eq!(err, "the image is 16385 bytes; the ATtiny1616 has 16384");
        let eeprom = vec![0x34; EEPROM_SIZE];
        let err = program(no_connection, &vec![0x12; 2 * FLASH_SIZE], Some(&eeprom)).unwrap_err();
        assert_eq!(err, "the image is 32768 bytes; the ATtiny1616 has 16384");
    }

    #[test]
    fn an_eeprom_image_bigger_than_eeprom_is_refused_before_the_port_is_opened() {
        let eeprom = vec![0x34; EEPROM_SIZE + 1];
        let err = program(no_connection, &[0x12], Some(&eeprom)).unwrap_err();
        assert_eq!(err, "the EEPROM image is bigger than 256 bytes");
    }

    #[test]
    fn images_that_fill_flash_and_eeprom_or_fall_short_of_them_are_written() {
        for (flash_len, eeprom_len) in [
            (FLASH_SIZE, EEPROM_SIZE),
            (FLASH_SIZE - 1, EEPROM_SIZE - 1),
            (1, 1),
            (0, 0),
        ] {
            let flash: Vec<u8> = (0..flash_len).map(|i| (i * 5 + 1) as u8 & 0x7F).collect();
            let eeprom: Vec<u8> = (0..eeprom_len).map(|i| (i * 3 + 2) as u8 & 0x7F).collect();
            let mut tiny = Tiny::new();
            tiny.flash.fill(0x42);

            let open = on(&mut tiny);
            program(
                || Programmer::connect_for_programming(open()?),
                &flash,
                Some(&eeprom),
            )
            .unwrap_or_else(|e| panic!("{flash_len} and {eeprom_len} bytes: {e}"));
            assert!(tiny.flash[..flash_len] == flash, "{flash_len} bytes");
            assert!(
                tiny.flash[flash_len..].iter().all(|&b| b == 0xFF),
                "{flash_len} bytes: the rest of flash isn't erased"
            );
            assert!(tiny.eeprom[..eeprom_len] == eeprom, "{eeprom_len} bytes");
            assert_eq!(tiny.lost_while_busy, 0);
            assert!(tiny.updi_disabled, "UPDI was left on");
        }
    }

    fn assert_let_go(tiny: &Tiny) {
        assert!(tiny.updi_disabled, "UPDI was left on");
        assert!(!tiny.progmode, "the chip was left in programming mode");
        assert!(!tiny.in_reset, "the chip was left in reset");
        assert_eq!(tiny.resets, 2, "one reset to enter and one to leave");
    }

    #[test]
    fn a_locked_chip_is_refused_and_keeps_what_it_holds() {
        let mut tiny = chip();
        tiny.locked = true;
        let (flash, eeprom) = (tiny.flash.clone(), tiny.eeprom.clone());

        let open = on(&mut tiny);
        let err = program(
            || Programmer::connect_for_programming(open()?),
            &[0x12; 200],
            Some(&[0x34; 40]),
        )
        .unwrap_err();
        assert!(err.contains("If it's locked"), "{err}");
        assert!(err.contains("which this tool won't do"), "{err}");
        assert!(tiny.flash == flash, "flash changed");
        assert!(tiny.eeprom == eeprom, "EEPROM changed");
        assert_eq!(tiny.page_writes + tiny.page_erase_writes, 0);
        assert_let_go(&tiny);
    }

    #[test]
    fn a_write_error_stops_the_flash_and_lets_the_chip_go() {
        let mut tiny = chip();
        tiny.write_error_at = Some(1);

        let open = on(&mut tiny);
        let err = program(
            || Programmer::connect_for_programming(open()?),
            &[0x12; 200],
            Some(&[0x34; 40]),
        )
        .unwrap_err();
        assert_eq!(err, "the NVM controller reported a write error");
        assert_eq!(tiny.page_writes, 1);
        assert_eq!(tiny.page_erase_writes, 0);
        assert_let_go(&tiny);
    }

    #[test]
    fn a_chip_that_stays_busy_is_given_up_on_and_let_go() {
        let mut tiny = chip();
        tiny.erase_time = std::time::Duration::MAX;

        let open = on(&mut tiny);
        let err = program(
            || Programmer::connect_for_programming(open()?),
            &[0x12; 200],
            None,
        )
        .unwrap_err();
        assert_eq!(err, "timed out waiting for the NVM controller");
        assert_eq!(tiny.page_writes, 0);
        assert_eq!(tiny.lost_while_busy, 0);
        assert_let_go(&tiny);
    }
}
