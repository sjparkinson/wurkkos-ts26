//! `cargo xtask <command>`: the AVR build and the steps around it that cargo
//! can't do.
//!
//! No dependencies beyond the firmware's own library. ELF reading, Intel HEX,
//! the UPDI programmer and the AVR simulator are all small enough to do by
//! hand, which saves needing avr-objcopy, avrdude, pymcuprog or simavr.

#[cfg(test)]
mod atdf;
mod avr;
mod crc;
mod elf;
mod ihex;
mod layout;
mod serial;
mod sim;
mod simulate;
mod spec;
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
use updi::{Programmer, map};

const USAGE: &str = "\
usage: cargo xtask <command>

commands:
  hex                     build the firmware, run it on the simulator, write
                          target/wurkkos-ts26-0721.hex and print flash, RAM,
                          stack, loop and handler use
  ping <port>             check the programmer can see the chip, writing nothing,
                          though expect it to reset as UPDI turns off
  peek <port>             read the running chip's registers and SRAM, to see how
                          far the firmware got; it writes nothing, and the same
                          reset follows, so only the first run counts
  backup <port>           save the torch's flash, EEPROM and fuses into backup/
  flash <port> [file.hex] build and flash the firmware, or flash file.hex
  restore <port>          put backup/flash.hex and backup/eeprom.hex back
  simulate [script...]    try src/ui.rs live in the terminal, or on a script
                          such as `1C 2H:1.5s wait:1s 3C`

<port> is the serial UPDI adapter, e.g. /dev/cu.usbserial-1410 on macOS
or /dev/ttyUSB0 on Linux.
";

const ELF: &str = "target/avr-none/release/ts26.elf";
/// src/bin/link_probe.rs, which `build` builds alongside the firmware.
const PROBE: &str = "target/avr-none/release/link-probe.elf";
const HEX: &str = "target/wurkkos-ts26-0721.hex";

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();

    let result = match args.as_slice() {
        ["hex"] => hex(),
        ["ping", port] => ping(port),
        ["peek", port] => peek(port),
        ["backup", port] => backup(port),
        ["flash", port] => build()
            .and_then(|(elf, _)| crc::sign(&elf.flash_image()))
            .and_then(|image| flash(port, &image)),
        ["flash", port, file] => this_boards_hex(Path::new(file))
            .and_then(|()| read_hex(Path::new(file)))
            .and_then(|image| signed(&image).map(|()| image))
            .and_then(|image| flash(port, &image)),
        ["restore", port] => restore(port),
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
    // xtask/ sits one below the workspace root.
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask has a parent directory")
        .to_path_buf()
}

/// Builds the firmware and the link probe, and refuses either if the layout
/// checks or the simulator fail.
fn build() -> Result<(elf::Elf, sim::Report), String> {
    let cargo = env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    run(Command::new(cargo).current_dir(root()).args([
        "build",
        "--package",
        "ts26",
        "--release",
        "--target",
        "avr-none",
        "-Zbuild-std=core",
    ]))?;
    let path = root().join(ELF);
    let bytes = fs::read(&path).map_err(at(&path))?;
    let elf = elf::Elf::parse(&bytes).map_err(at(&path))?;
    layout::check(&elf).map_err(at(&path))?;
    let report = sim::check(&elf).map_err(|e| format!("{}: simulator: {e}", path.display()))?;

    let probe = root().join(PROBE);
    let bytes = fs::read(&probe).map_err(at(&probe))?;
    let probe_elf = elf::Elf::parse(&bytes).map_err(at(&probe))?;
    probe_sections(&probe_elf).map_err(at(&probe))?;
    layout::memory(&probe_elf).map_err(at(&probe))?;
    sim::probe(&probe_elf).map_err(|e| format!("{}: simulator: {e}", probe.display()))?;
    Ok((elf, report))
}

/// The link probe is only worth checking if it really has something in
/// each section: `layout::memory` passes an empty .data without looking.
fn probe_sections(elf: &elf::Elf) -> Result<(), String> {
    for name in [".rodata", ".data", ".bss"] {
        match elf.section(name) {
            Some(range) if !range.is_empty() => {}
            _ => return Err(format!("the link probe has no {name}")),
        }
    }
    Ok(())
}

/// Writes the image `flash` would: the firmware padded to all of flash with
/// 0xFF and its CRC in the last two bytes (`crc.rs`). Then prints flash, RAM,
/// stack, loop and handler use.
fn hex() -> Result<(), String> {
    let (elf, sim) = build()?;
    let firmware = elf.flash_image();
    let image = crc::sign(&firmware)?;
    write(&root().join(HEX), &ihex::encode(&image))?;
    eprintln!("wrote {HEX}");

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

/// One line of the size report, in columns: used, out of, percent, unit.
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

/// `39062` as `39,062`.
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

fn connect(port: &str) -> Result<Programmer<Port>, String> {
    eprintln!("connecting on {port}");
    Programmer::connect(Port::open(port)?)
}

/// Connects, enters programming mode and refuses anything but an ATtiny1616.
/// Every command that reads or writes memory starts here.
fn connect_for_programming(port: &str) -> Result<Programmer<Port>, String> {
    let mut p = connect(port)?;
    p.enter_progmode()?;
    p.check_signature()?;
    Ok(p)
}

fn ping(port: &str) -> Result<(), String> {
    let mut p = connect(port)?;
    let sib = p.sib()?;
    p.disconnect()?;
    println!("found {}", String::from_utf8_lossy(&sib).trim_end());
    Ok(())
}

/// Reads what the running firmware has left behind, without a write: UPDI
/// reaches data space while the CPU runs. Turning UPDI off afterwards should
/// reset the chip (DS §33.3.2.2.2), so a second read sees a fresh boot, not what
/// the first one saw. Only `hw::init` sets the
/// clock, the pin directions and the PIT, so those say whether it ran.
/// VPORTB.OUT says whether the boost converter's on.
fn peek(port: &str) -> Result<(), String> {
    let registers = [
        ("RSTCTRL.RSTFR", sim::reg::RSTCTRL_RSTFR),
        ("CLKCTRL.MCLKCTRLB", sim::reg::CLKCTRL_MCLKCTRLB),
        ("WDT.CTRLA", sim::reg::WDT_CTRLA),
        ("CPU.SPL", usize::from(avr::SPL)),
        ("CPU.SPH", usize::from(avr::SPH)),
        ("CPU.SREG", usize::from(avr::SREG)),
        ("VPORTB.DIR", sim::reg::VPORTB_DIR),
        ("VPORTB.OUT", sim::reg::VPORTB_OUT),
        ("VPORTC.DIR", sim::reg::VPORTC_DIR),
        ("TCA0.CTRLA", sim::reg::TCA0_CTRLA),
        ("TCA0.CTRLB", sim::reg::TCA0_CTRLB),
        ("RTC.PITCTRLA", sim::reg::RTC_PITCTRLA),
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

/// `bytes` as one line per run of equal bytes, so a pattern and where it stops
/// stand out from the 2 KB around it.
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

/// Refuses a file whose name doesn't say it's for this board. An image has no
/// model in it, so the name's all there is. It catches picking up the TS21's
/// hex by mistake, not a file that's been renamed.
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
         file `cargo xtask hex` writes, or use `restore` for a backup",
        path.display()
    ))
}

/// Checks a file's image against the CRC our firmware checks at every power-on,
/// since one that doesn't match would flash and verify cleanly, then cap itself
/// at level 50 with nothing to say why. `cargo xtask hex` always writes all
/// 16 KB, so a full image that doesn't check is refused. A shorter one isn't
/// ours, or wasn't signed: Anduril's hex is like that and needs no CRC, so it
/// gets a warning and goes ahead. Not for `restore`, since a factory backup has
/// no CRC of ours.
fn signed(image: &[u8]) -> Result<(), String> {
    if image.len() < map::FLASH_SIZE {
        eprintln!(
            "warning: the image isn't the full {} bytes with a CRC, so it isn't a signed build \
             of this firmware. That's right for Anduril; if it's ours, it'll cap itself at \
             level 50 on every boot, so flash the file `cargo xtask hex` writes instead",
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

fn backup(port: &str) -> Result<(), String> {
    let dir = root().join("backup");
    let flash_path = dir.join("flash.hex");
    if flash_path.exists() {
        return Err(format!(
            "{} already exists. Move it somewhere safe first; if it's the factory \
             firmware, it's the only copy",
            flash_path.display()
        ));
    }

    let mut p = connect_for_programming(port)?;
    eprintln!("reading flash");
    let flash = p.read(map::FLASH, map::FLASH_SIZE)?;
    eprintln!("reading EEPROM and fuses");
    let eeprom = p.read(map::EEPROM, map::EEPROM_SIZE)?;
    let fuses = p.read(map::FUSES, map::FUSES_SIZE)?;
    p.leave()?;

    fs::create_dir_all(&dir).map_err(at(&dir))?;
    write(&flash_path, &ihex::encode(&flash))?;
    write(&dir.join("eeprom.hex"), &ihex::encode(&eeprom))?;
    write(&dir.join("fuses.hex"), &ihex::encode(&fuses))?;
    eprintln!("saved flash, EEPROM and fuses (for reference, never written back) to backup/");
    Ok(())
}

fn flash(port: &str, image: &[u8]) -> Result<(), String> {
    program(port, image, None)
}

fn restore(port: &str) -> Result<(), String> {
    let dir = root().join("backup");
    let flash = read_hex(&dir.join("flash.hex"))?;
    let eeprom = read_hex(&dir.join("eeprom.hex"))?;
    program(port, &flash, Some(&eeprom))
}

/// Erase, write, verify, restart. The chip erase wipes EEPROM too unless the
/// EESAVE fuse is set; pass `Some` to put contents back.
fn program(port: &str, flash: &[u8], eeprom: Option<&[u8]>) -> Result<(), String> {
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

    let mut p = connect_for_programming(port)?;
    let written = write_and_verify(&mut p, flash, eeprom);
    // Leave programming mode even after a failure, so the chip isn't held in
    // it until the next power cycle. The first error is the one to report.
    let left = p.leave();
    written?;
    left?;
    eprintln!("verified and restarted");
    Ok(())
}

fn write_and_verify(
    p: &mut Programmer<Port>,
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

/// Prefixes an error with the file it came from.
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
    use std::path::Path;

    use super::{commas, crc, ihex, row, runs, signed, this_boards_hex};

    #[test]
    fn flash_takes_only_a_signed_image() {
        let image = crc::sign(&[0x12, 0x34, 0x56]).unwrap();
        let round_trip = ihex::decode(&ihex::encode(&image)).unwrap();
        assert!(signed(&round_trip).is_ok());

        let mut broken = image.clone();
        broken[1] ^= 1;
        assert!(signed(&broken).is_err());
    }

    #[test]
    fn flash_still_takes_a_short_image_such_as_anduril() {
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
    fn flash_takes_this_boards_hex_and_refuses_the_ts21s() {
        for good in [
            "target/wurkkos-ts26-0721.hex",
            "anduril.wurkkos-ts26.hex",
            "WURKKOS-TS26.HEX",
        ] {
            assert!(this_boards_hex(Path::new(good)).is_ok(), "{good}");
        }
        for bad in [
            "target/wurkkos-ts21-0614.hex",
            "backup/flash.hex",
            "ts26/firmware.hex",
        ] {
            let err = this_boards_hex(Path::new(bad)).unwrap_err();
            assert!(err.contains("model 0721"), "{bad}: {err}");
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
}
