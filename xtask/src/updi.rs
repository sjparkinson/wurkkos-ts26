//! A UPDI programmer for the ATtiny1616 over a serial adapter (DS40002204A
//! chapter 33), with the sequences of pymcuprog.
//!
//! It deliberately can't write fuses, write the user row or unlock a locked
//! chip. A wrong fuse can turn UPDI off for good.

use std::fmt;
use std::io;
use std::path::Path;
use std::time::{Duration, Instant};

use ts26::regs::{
    NVMCTRL_CMD_CHIPERASE as NVM_CHIP_ERASE, NVMCTRL_CMD_PAGEBUFCLR as NVM_PAGE_BUFFER_CLEAR,
    NVMCTRL_CMD_PAGEERASEWRITE as NVM_PAGE_ERASE_WRITE, NVMCTRL_CMD_PAGEWRITE as NVM_PAGE_WRITE,
    NVMCTRL_EEBUSY as NVM_EEBUSY, NVMCTRL_FBUSY as NVM_FBUSY, NVMCTRL_WRERROR as NVM_WRERROR,
};

pub mod map {
    use ts26::regs::{self, addr, mem};

    /// Writes land in the page buffer until an NVM command commits them.
    pub const FLASH: u16 = mem::MAPPED_PROGMEM_START as u16;
    pub const FLASH_SIZE: usize = mem::MAPPED_PROGMEM_SIZE;
    pub const FLASH_PAGE: usize = mem::MAPPED_PROGMEM_PAGE;

    pub const EEPROM: u16 = mem::EEPROM_START as u16;
    pub const EEPROM_SIZE: usize = mem::EEPROM_SIZE;
    pub const EEPROM_PAGE: usize = mem::EEPROM_PAGE;

    pub const SIGNATURE: u16 = mem::SIGNATURES_START as u16;
    pub const ATTINY1616: [u8; 3] = regs::SIGNATURE;

    /// Read only, since UPDI can't write the signature row (DS table 6-5).
    pub const SERIAL: u16 = addr::SIGROW_SERNUM0 as u16;
    pub const SERIAL_SIZE: usize = addr::SIGROW_SERNUM9 - addr::SIGROW_SERNUM0 + 1;

    /// The fuses, then the lock byte. Read for `backup`, and nothing here
    /// writes them.
    pub const FUSES: u16 = mem::FUSES_START as u16;
    pub const FUSES_SIZE: usize = mem::LOCKBITS_END - mem::FUSES_START;

    pub const NVMCTRL_CTRLA: u16 = addr::NVMCTRL_CTRLA as u16;
    pub const NVMCTRL_STATUS: u16 = addr::NVMCTRL_STATUS as u16;
}

/// `send` swallows the echo of what it writes.
pub trait Wire {
    fn send(&mut self, bytes: &[u8]) -> io::Result<()>;
    fn recv(&mut self, buf: &mut [u8]) -> io::Result<()>;
    /// Resets UPDI's own state machine, not the chip.
    fn double_break(&mut self) -> io::Result<()>;
    fn now(&self) -> Instant {
        Instant::now()
    }
}

pub type Result<T> = std::result::Result<T, String>;

pub fn fits_flash(data: &[u8]) -> Result<()> {
    if data.len() > map::FLASH_SIZE {
        return Err(format!(
            "the image is {} bytes; the ATtiny1616 has {}",
            data.len(),
            map::FLASH_SIZE
        ));
    }
    Ok(())
}

const SYNC: u8 = 0x55;
const ACK: u8 = 0x40;

const LDS: u8 = 0x00;
const LD: u8 = 0x20;
const STS: u8 = 0x40;
const ST: u8 = 0x60;
const LDCS: u8 = 0x80;
const REPEAT: u8 = 0xA0;
const STCS: u8 = 0xC0;
const KEY: u8 = 0xE0;

const PTR_INC: u8 = 0x04;
const PTR_ADDRESS: u8 = 0x08;
const ADDRESS_16: u8 = 0x04;
const DATA_8: u8 = 0x00;
const DATA_16: u8 = 0x01;
const KEY_SIB: u8 = 0x04;
const KEY_64: u8 = 0x00;
const SIB_16: u8 = 0x01;

const CS_STATUSA: u8 = 0x00;
const CS_CTRLA: u8 = 0x02;
const CS_CTRLB: u8 = 0x03;
const ASI_KEY_STATUS: u8 = 0x07;
const ASI_RESET_REQ: u8 = 0x08;
const ASI_SYS_STATUS: u8 = 0x0B;

const CTRLA_IBDLY: u8 = 1 << 7; // inter-byte delay on replies
const CTRLA_RSD: u8 = 1 << 3; // response signature (ACK) disable
const CTRLB_CCDETDIS: u8 = 1 << 3; // collision detection off
const CTRLB_UPDIDIS: u8 = 1 << 2; // turn UPDI off
const KEY_STATUS_NVMPROG: u8 = 1 << 4;
const SYS_STATUS_NVMPROG: u8 = 1 << 3;
const SYS_STATUS_LOCKSTATUS: u8 = 1 << 0;
const RESET_REQ_APPLY: u8 = 0x59;

const KEY_NVMPROG: &[u8; 8] = b"NVMProg ";

const MAX_BLOCK: usize = 256;

const PROGMODE_TIMEOUT: Duration = Duration::from_millis(500);

/// A unique ID by DS §6.10.2.2, unconfirmed across chips.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SerialNumber([u8; map::SERIAL_SIZE]);

impl SerialNumber {
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        match bytes.try_into() {
            Ok(bytes) => Ok(Self(bytes)),
            Err(_) => Err(format!(
                "a serial number is {} bytes, not {}",
                map::SERIAL_SIZE,
                bytes.len()
            )),
        }
    }

    pub const fn bytes(&self) -> &[u8] {
        &self.0
    }

    /// Nothing says a real chip ever reads like this.
    pub fn is_blank(&self) -> bool {
        self.0 == [0xFF; map::SERIAL_SIZE] || self.0 == [0x00; map::SERIAL_SIZE]
    }

    pub fn check_saved(&self, dir: &Path) -> Result<()> {
        if !self.is_blank() {
            return Ok(());
        }
        Err(format!(
            "the backup in {} was saved with a serial number of all {:02X}, which can't tell \
             one chip from another. `restore` only writes a backup onto the chip it was read \
             from, and with that serial number it can't tell, so it won't write this backup",
            dir.display(),
            self.0[0]
        ))
    }

    /// Asked before anything's erased, which is what the errors say.
    fn check_chip(&self, chip: &Self, dir: &Path) -> Result<()> {
        self.check_saved(dir)?;
        if chip.is_blank() {
            return Err(format!(
                "this chip's serial number reads all {:02X}, which can't tell one chip from \
                 another, so `restore` can't tell whether the backup in {} (serial number \
                 {self}) was read from it. Nothing was erased or written",
                chip.0[0],
                dir.display()
            ));
        }
        if chip != self {
            return Err(format!(
                "this chip's serial number is {chip}, and the backup in {} was read from a \
                 chip whose serial number is {self}. `restore` only writes a backup onto the \
                 chip it was read from, so nothing was erased or written",
                dir.display()
            ));
        }
        Ok(())
    }
}

impl fmt::Display for SerialNumber {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&hex(&self.0))
    }
}

pub struct Programmer<W: Wire> {
    wire: W,
}

impl<W: Wire> Programmer<W> {
    /// Gives up after a second failure without sending UPDIDIS on purpose. A
    /// line that's failed twice gets no more bytes, and UPDI left on only
    /// costs power (DS §33.3.2.2.2) until the probe lifts.
    pub fn connect(wire: W) -> Result<Self> {
        let mut p = Self { wire };
        if p.init_link().is_err() {
            p.wire.double_break().map_err(io_error)?;
            p.init_link()?;
        }
        Ok(p)
    }

    /// On failure the NVM key and a reset may already have gone out, so it
    /// leaves before reporting. Leaving only writes UPDI's own registers,
    /// which is all a locked chip accepts (DS §33.3.8.1), and erases nothing.
    pub fn connect_for_programming(wire: W) -> Result<Self> {
        let mut p = Self::connect(wire)?;
        match p.enter_progmode().and_then(|()| p.check_signature()) {
            Ok(()) => Ok(p),
            Err(error) => {
                // Whatever leaving says, the first error is the one to report.
                let _ = p.leave();
                Err(error)
            }
        }
    }

    /// No programmer to erase with until the serial numbers match.
    pub fn connect_for_restore(wire: W, saved: &SerialNumber, dir: &Path) -> Result<Self> {
        saved.check_saved(dir)?;
        let mut p = Self::connect_for_programming(wire)?;
        match p
            .serial_number()
            .and_then(|chip| saved.check_chip(&chip, dir))
        {
            Ok(()) => Ok(p),
            Err(error) => {
                // Whatever leaving says, the first error is the one to report.
                let _ = p.leave();
                Err(error)
            }
        }
    }

    fn init_link(&mut self) -> Result<()> {
        self.stcs(CS_CTRLB, CTRLB_CCDETDIS)?;
        self.stcs(CS_CTRLA, CTRLA_IBDLY)?;
        match self.ldcs(CS_STATUSA)? {
            0 => Err("the UPDI link came up but STATUSA reads 0".into()),
            _ => Ok(()),
        }
    }

    pub fn sib(&mut self) -> Result<[u8; 16]> {
        self.send(&[SYNC, KEY | KEY_SIB | SIB_16])?;
        let mut sib = [0; 16];
        self.recv(&mut sib)?;
        Ok(sib)
    }

    pub fn enter_progmode(&mut self) -> Result<()> {
        // NVMPROG needs the lock bits cleared (DS40002204A table 33-4), but
        // nothing says a chip can't read both set.
        let status = self.ldcs(ASI_SYS_STATUS)?;
        if status & SYS_STATUS_LOCKSTATUS == 0 && status & SYS_STATUS_NVMPROG != 0 {
            return Ok(());
        }

        let mut frame = vec![SYNC, KEY | KEY_64];
        frame.extend(KEY_NVMPROG.iter().rev());
        self.send(&frame)?;
        if self.ldcs(ASI_KEY_STATUS)? & KEY_STATUS_NVMPROG == 0 {
            return Err("the chip didn't accept the NVM programming key".into());
        }

        // Timed by the clock, since a fast adapter could run out of polls
        // inside the start-up time. SUT is 64 ms at most (ATDF), and avrdude
        // allows 500 ms.
        self.reset()?;
        let deadline = self.wire.now() + PROGMODE_TIMEOUT;
        loop {
            let status = self.ldcs(ASI_SYS_STATUS)?;
            if status & SYS_STATUS_LOCKSTATUS == 0 && status & SYS_STATUS_NVMPROG != 0 {
                return Ok(());
            }
            if self.wire.now() >= deadline {
                break;
            }
        }
        Err(
            "the chip didn't enter programming mode. If it's locked, unlocking means \
             erasing it, which this tool won't do"
                .into(),
        )
    }

    pub fn check_signature(&mut self) -> Result<()> {
        let signature = self.read(map::SIGNATURE, 3)?;
        if signature == map::ATTINY1616 {
            Ok(())
        } else {
            Err(format!(
                "expected an ATtiny1616 (signature {}) but found {}",
                hex(&map::ATTINY1616),
                hex(&signature)
            ))
        }
    }

    pub fn serial_number(&mut self) -> Result<SerialNumber> {
        let bytes = self.read(map::SERIAL, map::SERIAL_SIZE)?;
        SerialNumber::from_bytes(&bytes)
    }

    pub fn read(&mut self, addr: u16, len: usize) -> Result<Vec<u8>> {
        let mut out = vec![0; len];
        for (i, chunk) in out.chunks_mut(MAX_BLOCK).enumerate() {
            self.st_ptr(offset(addr, i * MAX_BLOCK)?)?;
            let mut frame = repeat(chunk.len());
            frame.extend([SYNC, LD | PTR_INC | DATA_8]);
            self.send(&frame)?;
            self.recv(chunk)?;
        }
        Ok(out)
    }

    /// EEPROM too, unless the EESAVE fuse is set.
    pub fn chip_erase(&mut self) -> Result<()> {
        self.wait_nvm()?;
        self.nvm_command(NVM_CHIP_ERASE)?;
        self.wait_nvm()
    }

    /// One call, because a page write can only clear bits, so over flash that
    /// isn't blank it ANDs the new data into the old.
    pub fn erase_and_write_flash(&mut self, data: &[u8]) -> Result<()> {
        if data.len() > map::FLASH_SIZE {
            return Err(format!(
                "the image is {} bytes; the ATtiny1616 has {}",
                data.len(),
                map::FLASH_SIZE
            ));
        }
        self.chip_erase()?;
        self.write_pages(map::FLASH, map::FLASH_PAGE, data, NVM_PAGE_WRITE, true)
    }

    /// `addr` must be page aligned. EESAVE can keep EEPROM through a chip
    /// erase, so a page that should end up blank still has to be written.
    pub fn write(&mut self, addr: u16, page: usize, data: &[u8]) -> Result<()> {
        self.write_pages(addr, page, data, NVM_PAGE_ERASE_WRITE, false)
    }

    fn write_pages(
        &mut self,
        addr: u16,
        page: usize,
        data: &[u8],
        command: u8,
        skip_blank: bool,
    ) -> Result<()> {
        self.wait_nvm()?;
        for (i, chunk) in data.chunks(page).enumerate() {
            if skip_blank && chunk.iter().all(|&b| b == 0xFF) {
                continue;
            }
            self.write_page(offset(addr, i * page)?, chunk, command)?;
        }
        Ok(())
    }

    pub fn verify(&mut self, addr: u16, data: &[u8]) -> Result<()> {
        let actual = self.read(addr, data.len())?;
        match actual.iter().zip(data).position(|(a, b)| a != b) {
            None => Ok(()),
            Some(i) => Err(format!(
                "verify failed at 0x{:04X}: wrote {:02X}, read {:02X}",
                usize::from(addr) + i,
                data[i],
                actual[i]
            )),
        }
    }

    pub fn leave(mut self) -> Result<W> {
        self.reset()?;
        self.disconnect()
    }

    /// UPDIDIS issues a system reset (DS §33.3.2.2.2), so the chip restarts.
    /// Unconfirmed on a real one.
    pub fn disconnect(mut self) -> Result<W> {
        self.stcs(CS_CTRLB, CTRLB_UPDIDIS | CTRLB_CCDETDIS)?;
        Ok(self.wire)
    }

    /// Expects the NVM controller to be idle.
    fn write_page(&mut self, addr: u16, data: &[u8], command: u8) -> Result<()> {
        self.nvm_command(NVM_PAGE_BUFFER_CLEAR)?;
        self.wait_nvm()?;

        self.st_ptr(addr)?;
        let mut frame = vec![SYNC, STCS | CS_CTRLA, CTRLA_IBDLY | CTRLA_RSD];
        frame.extend(repeat(data.len()));
        frame.extend([SYNC, ST | PTR_INC | DATA_8]);
        frame.extend(data);
        frame.extend([SYNC, STCS | CS_CTRLA, CTRLA_IBDLY]);
        self.send(&frame)?;

        self.nvm_command(command)?;
        self.wait_nvm()
    }

    /// A count of polls, not the clock: typical times are 4 ms at most (DS
    /// table 36-38, with no maximum), and 10,000 polls take a second even at
    /// 0.1 ms a round trip.
    fn wait_nvm(&mut self) -> Result<()> {
        for _ in 0..10_000 {
            let status = self.lds(map::NVMCTRL_STATUS)?;
            if status & NVM_WRERROR != 0 {
                return Err("the NVM controller reported a write error".into());
            }
            if status & (NVM_FBUSY | NVM_EEBUSY) == 0 {
                return Ok(());
            }
        }
        Err("timed out waiting for the NVM controller".into())
    }

    fn nvm_command(&mut self, command: u8) -> Result<()> {
        self.sts(map::NVMCTRL_CTRLA, command)
    }

    fn reset(&mut self) -> Result<()> {
        self.stcs(ASI_RESET_REQ, RESET_REQ_APPLY)?;
        self.stcs(ASI_RESET_REQ, 0)
    }

    fn ldcs(&mut self, reg: u8) -> Result<u8> {
        self.send(&[SYNC, LDCS | reg])?;
        self.recv1()
    }

    fn stcs(&mut self, reg: u8, value: u8) -> Result<()> {
        self.send(&[SYNC, STCS | reg, value])
    }

    fn lds(&mut self, addr: u16) -> Result<u8> {
        let [lo, hi] = addr.to_le_bytes();
        self.send(&[SYNC, LDS | ADDRESS_16 | DATA_8, lo, hi])?;
        self.recv1()
    }

    fn sts(&mut self, addr: u16, value: u8) -> Result<()> {
        let [lo, hi] = addr.to_le_bytes();
        self.send(&[SYNC, STS | ADDRESS_16 | DATA_8, lo, hi])?;
        self.ack()?;
        self.send(&[value])?;
        self.ack()
    }

    fn st_ptr(&mut self, addr: u16) -> Result<()> {
        let [lo, hi] = addr.to_le_bytes();
        self.send(&[SYNC, ST | PTR_ADDRESS | DATA_16, lo, hi])?;
        self.ack()
    }

    fn ack(&mut self) -> Result<()> {
        match self.recv1()? {
            ACK => Ok(()),
            other => Err(format!(
                "expected an ACK (0x40) from the chip, got 0x{other:02X}"
            )),
        }
    }

    fn recv1(&mut self) -> Result<u8> {
        let mut byte = [0];
        self.recv(&mut byte)?;
        Ok(byte[0])
    }

    fn send(&mut self, bytes: &[u8]) -> Result<()> {
        self.wire.send(bytes).map_err(io_error)
    }

    fn recv(&mut self, buf: &mut [u8]) -> Result<()> {
        self.wire.recv(buf).map_err(io_error)
    }
}

fn offset(addr: u16, by: usize) -> Result<u16> {
    u16::try_from(usize::from(addr) + by).map_err(|_| "address out of range".to_string())
}

fn repeat(count: usize) -> Vec<u8> {
    assert!((1..=MAX_BLOCK).contains(&count), "REPEAT covers 1 to 256");
    vec![SYNC, REPEAT, (count - 1) as u8]
}

fn hex(bytes: &[u8]) -> String {
    let hex: Vec<String> = bytes.iter().map(|b| format!("{b:02X}")).collect();
    hex.join(" ")
}

fn io_error(error: io::Error) -> String {
    match error.kind() {
        io::ErrorKind::TimedOut => "no reply from the chip. Check the probe's contact, \
                                    the pad order and that the cell is out"
            .into(),
        _ => format!("serial: {error}"),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use fake::Tiny;

    /// Nothing's read from it.
    const BACKUP: &str = "backup/breakout";

    #[test]
    fn map_assumes_what_the_atdf_says() {
        use ts26::regs::{addr, mem};

        let sernum = [
            addr::SIGROW_SERNUM0,
            addr::SIGROW_SERNUM1,
            addr::SIGROW_SERNUM2,
            addr::SIGROW_SERNUM3,
            addr::SIGROW_SERNUM4,
            addr::SIGROW_SERNUM5,
            addr::SIGROW_SERNUM6,
            addr::SIGROW_SERNUM7,
            addr::SIGROW_SERNUM8,
            addr::SIGROW_SERNUM9,
        ];
        assert_eq!(sernum.len(), map::SERIAL_SIZE, "serial number size");
        for (n, at) in sernum.into_iter().enumerate() {
            assert_eq!(at, usize::from(map::SERIAL) + n, "SERNUM{n}");
        }
        assert_eq!(
            mem::SIGNATURES_END,
            usize::from(map::SERIAL),
            "serial number follows the signature"
        );

        assert_eq!(map::FLASH_SIZE, mem::PROGMEM_SIZE, "PROGMEM size");
        assert_eq!(map::FLASH_PAGE, mem::PROGMEM_PAGE, "PROGMEM page");
        assert_eq!(
            map::ATTINY1616.len(),
            mem::SIGNATURES_SIZE,
            "signature size"
        );
        assert_eq!(
            mem::FUSES_END,
            mem::LOCKBITS_START,
            "lock byte follows the fuses"
        );
    }

    /// Pinned to DS40002204 §6, so a generator that goes wrong can't move a
    /// real chip's writes.
    #[test]
    fn map_is_the_datasheets() {
        assert_eq!(map::FLASH, 0x8000, "flash window");
        assert_eq!(map::FLASH_SIZE, 16 * 1024, "flash size");
        assert_eq!(map::FLASH_PAGE, 64, "flash page");
        assert_eq!(map::EEPROM, 0x1400, "EEPROM");
        assert_eq!(map::EEPROM_SIZE, 256, "EEPROM size");
        assert_eq!(map::EEPROM_PAGE, 32, "EEPROM page");
        assert_eq!(map::SIGNATURE, 0x1100, "signature");
        assert_eq!(map::ATTINY1616, [0x1E, 0x94, 0x21], "ATtiny1616");
        // SIGROW is at 0x1100 (§7.1) and SERNUM0 is 3 into it (§6.10.1).
        assert_eq!(map::SERIAL, 0x1103, "serial number");
        assert_eq!(map::SERIAL_SIZE, 10, "serial number size");
        assert_eq!(map::FUSES, 0x1280, "fuses");
        assert_eq!(map::FUSES_SIZE, 11, "fuses and lock byte");
        assert_eq!(map::NVMCTRL_CTRLA, 0x1000, "NVMCTRL.CTRLA");
        assert_eq!(map::NVMCTRL_STATUS, 0x1002, "NVMCTRL.STATUS");
    }

    /// From the NVMCTRL chapter and pymcuprog's `nvmp0.py`. The fake chip
    /// decodes the same names, so a wrong alias, FUSEWRITE in particular,
    /// would pass every other test.
    #[test]
    fn nvm_commands_are_the_datasheets() {
        assert_eq!(NVM_PAGE_WRITE, 0x01, "WP");
        assert_eq!(NVM_PAGE_ERASE_WRITE, 0x03, "ERWP");
        assert_eq!(NVM_PAGE_BUFFER_CLEAR, 0x04, "PBC");
        assert_eq!(NVM_CHIP_ERASE, 0x05, "CHER");
        assert_eq!(NVM_FBUSY, 0x01, "FBUSY");
        assert_eq!(NVM_EEBUSY, 0x02, "EEBUSY");
        assert_eq!(NVM_WRERROR, 0x04, "WRERROR");
    }

    #[test]
    fn reads_the_sib() {
        let mut p = Programmer::connect(Tiny::new()).unwrap();
        assert_eq!(&p.sib().unwrap(), b"tinyAVR P:0D:0-3");
    }

    #[test]
    fn reads_data_space_while_the_firmware_runs() {
        let mut p = Programmer::connect(Tiny::new()).unwrap();
        assert_eq!(p.read(map::SIGNATURE, 3).unwrap(), map::ATTINY1616);
        p.disconnect().unwrap();
    }

    #[test]
    fn retries_with_a_double_break() {
        let mut tiny = Tiny::new();
        tiny.asleep = true;
        let tiny = Programmer::connect(tiny).unwrap().disconnect().unwrap();
        assert_eq!(tiny.breaks, 1);
        assert!(tiny.updi_disabled);
    }

    #[test]
    fn enters_progmode_and_checks_the_signature() {
        let mut p = Programmer::connect(Tiny::new()).unwrap();
        p.enter_progmode().unwrap();
        p.check_signature().unwrap();
    }

    #[test]
    fn waits_for_the_chip_to_start_up_on_a_fast_or_slow_adapter() {
        for round_trip in [Duration::from_micros(100), Duration::from_millis(16)] {
            let mut tiny = Tiny::new();
            tiny.start_up = Duration::from_millis(400);
            tiny.round_trip = round_trip;
            let mut p = Programmer::connect(tiny).unwrap();
            p.enter_progmode()
                .unwrap_or_else(|e| panic!("{round_trip:?} a round trip: {e}"));
            p.check_signature().unwrap();
        }
    }

    #[test]
    fn gives_up_on_the_clock_not_a_count_of_polls() {
        for round_trip in [Duration::from_micros(100), Duration::from_millis(16)] {
            let mut tiny = Tiny::new();
            tiny.start_up = Duration::from_millis(600);
            tiny.round_trip = round_trip;
            let mut p = Programmer::connect(tiny).unwrap();
            let before = p.wire.elapsed;
            assert!(p.enter_progmode().unwrap_err().contains("programming mode"));
            let waited = p.wire.elapsed.checked_sub(before).unwrap();
            assert!(
                (PROGMODE_TIMEOUT..PROGMODE_TIMEOUT + 4 * round_trip).contains(&waited),
                "{round_trip:?} a round trip: gave up after {waited:?}"
            );
        }
    }

    #[test]
    fn refuses_another_chip() {
        let mut tiny = Tiny::new();
        tiny.signature = [0x1E, 0x94, 0x20];
        let mut p = Programmer::connect(tiny).unwrap();
        p.enter_progmode().unwrap();
        assert!(p.check_signature().unwrap_err().contains("1E 94 20"));
    }

    #[test]
    fn refuses_a_locked_chip() {
        let mut tiny = Tiny::new();
        tiny.locked = true;
        let mut p = Programmer::connect(tiny).unwrap();
        assert!(p.enter_progmode().unwrap_err().contains("locked"));
    }

    #[test]
    fn connects_for_programming_and_stays_connected() {
        let mut tiny = Tiny::new();
        let p = Programmer::connect_for_programming(&mut tiny);
        assert!(p.is_ok());
        drop(p);
        assert!(tiny.progmode, "not in programming mode");
        assert!(!tiny.updi_disabled, "UPDI was turned off");
    }

    fn with_contents(mut tiny: Tiny) -> Tiny {
        tiny.flash.fill(0x42);
        tiny.eeprom.fill(0x24);
        tiny
    }

    fn assert_let_go(tiny: &Tiny) {
        assert_eq!(tiny.resets, 2, "one reset to enter and one to leave");
        assert!(!tiny.in_reset, "the chip was left in reset");
        assert!(!tiny.progmode, "the chip was left in programming mode");
        assert!(tiny.updi_disabled, "UPDI was left on");
        assert!(tiny.flash.iter().all(|&b| b == 0x42), "flash changed");
        assert!(tiny.eeprom.iter().all(|&b| b == 0x24), "EEPROM changed");
    }

    fn refused(tiny: Tiny) -> String {
        let mut tiny = with_contents(tiny);
        let Err(error) = Programmer::connect_for_programming(&mut tiny) else {
            panic!("connected for programming");
        };
        assert_let_go(&tiny);
        error
    }

    fn refused_a_restore(tiny: Tiny, saved: &SerialNumber) -> String {
        let mut tiny = with_contents(tiny);
        let Err(error) = Programmer::connect_for_restore(&mut tiny, saved, Path::new(BACKUP))
        else {
            panic!("connected for a restore");
        };
        assert_let_go(&tiny);
        error
    }

    #[test]
    fn a_wrong_signature_ends_with_updi_off() {
        let mut tiny = Tiny::new();
        tiny.signature = [0x1E, 0x94, 0x20];
        let err = refused(tiny);
        assert!(err.contains("1E 94 20"), "{err}");
    }

    #[test]
    fn a_start_up_that_never_finishes_ends_with_updi_off() {
        let mut tiny = Tiny::new();
        tiny.start_up = Duration::from_secs(3600);
        let err = refused(tiny);
        assert!(err.contains("programming mode"), "{err}");
    }

    #[test]
    fn a_locked_chip_ends_with_updi_off_and_nothing_written() {
        let mut tiny = Tiny::new();
        tiny.locked = true;
        let err = refused(tiny);
        assert!(err.contains("locked"), "{err}");
    }

    #[test]
    fn reads_the_serial_number_from_sernum0_to_sernum9() {
        let mut tiny = Tiny::new();
        tiny.serial = [0xA0, 0xA1, 0xA2, 0xA3, 0xA4, 0xA5, 0xA6, 0xA7, 0xA8, 0xA9];
        let mut p = Programmer::connect_for_programming(tiny).unwrap();
        let serial = p.serial_number().unwrap();
        assert_eq!(serial.bytes(), p.wire.serial);
        assert_eq!(serial.to_string(), "A0 A1 A2 A3 A4 A5 A6 A7 A8 A9");
    }

    #[test]
    fn a_serial_number_is_ten_bytes() {
        assert!(SerialNumber::from_bytes(&[0x12; 10]).is_ok());
        for wrong in [0, 9, 11] {
            let err = SerialNumber::from_bytes(&vec![0x12; wrong]).unwrap_err();
            assert!(err.contains(&format!("not {wrong}")), "{err}");
        }
    }

    #[test]
    fn a_serial_number_of_all_ff_or_all_00_is_blank() {
        for fill in [0xFF, 0x00] {
            assert!(SerialNumber([fill; map::SERIAL_SIZE]).is_blank());
            for n in 0..map::SERIAL_SIZE {
                let mut bytes = [fill; map::SERIAL_SIZE];
                bytes[n] ^= 0x01;
                assert!(!SerialNumber(bytes).is_blank(), "{fill:02X}, byte {n}");
            }
        }
        assert!(!SerialNumber(Tiny::new().serial).is_blank(), "the fake's");
    }

    #[test]
    fn connects_for_a_restore_onto_the_chip_the_backup_was_read_from() {
        let mut tiny = Tiny::new();
        let saved = SerialNumber(tiny.serial);
        let p = Programmer::connect_for_restore(&mut tiny, &saved, Path::new(BACKUP));
        assert!(p.is_ok());
        drop(p);
        assert!(tiny.progmode, "not in programming mode");
        assert!(!tiny.updi_disabled, "UPDI was turned off");
    }

    #[test]
    fn a_restore_onto_another_chip_ends_with_updi_off_and_nothing_written() {
        let saved = SerialNumber(Tiny::new().serial);
        for n in 0..map::SERIAL_SIZE {
            let mut tiny = Tiny::new();
            tiny.serial[n] ^= 0x01;
            let chip = SerialNumber(tiny.serial);
            let err = refused_a_restore(tiny, &saved);
            assert!(err.contains(&chip.to_string()), "byte {n}: {err}");
            assert!(err.contains(&saved.to_string()), "byte {n}: {err}");
            assert!(err.contains(BACKUP), "byte {n}: {err}");
        }
    }

    #[test]
    fn a_restore_onto_a_chip_with_a_blank_serial_number_ends_with_updi_off() {
        let saved = SerialNumber(Tiny::new().serial);
        for (blank, name) in [(0xFF, "all FF"), (0x00, "all 00")] {
            let mut tiny = Tiny::new();
            tiny.serial = [blank; map::SERIAL_SIZE];
            let err = refused_a_restore(tiny, &saved);
            assert!(err.contains(name), "{err}");
            assert!(err.contains(&saved.to_string()), "{err}");
            assert!(err.contains(BACKUP), "{err}");
        }
    }

    #[test]
    fn a_restore_of_a_backup_with_a_blank_serial_number_sends_nothing() {
        for (blank, name) in [(0xFF, "all FF"), (0x00, "all 00")] {
            let mut tiny = Tiny::new();
            tiny.serial = [blank; map::SERIAL_SIZE];
            let saved = SerialNumber(tiny.serial);
            let Err(err) = Programmer::connect_for_restore(&mut tiny, &saved, Path::new(BACKUP))
            else {
                panic!("connected for a restore");
            };
            assert!(err.contains(name), "{err}");
            assert!(err.contains(BACKUP), "{err}");
            assert_eq!(tiny.sent, 0, "bytes went out");
        }
    }

    #[test]
    fn a_restore_checks_the_signature_before_the_serial_number() {
        let mut tiny = Tiny::new();
        tiny.signature = [0x1E, 0x94, 0x20];
        tiny.serial[0] ^= 0x01;
        let err = refused_a_restore(tiny, &SerialNumber(Tiny::new().serial));
        assert!(err.contains("1E 94 20"), "{err}");
    }

    #[test]
    fn writes_and_verifies_flash_then_starts_the_firmware() {
        let image: Vec<u8> = (0..1000u32).map(|i| (i * 7 + 3) as u8).collect();
        let mut tiny = Tiny::new();
        tiny.flash.fill(0x42);

        let mut p = Programmer::connect(tiny).unwrap();
        p.enter_progmode().unwrap();
        p.erase_and_write_flash(&image).unwrap();
        p.verify(map::FLASH, &image).unwrap();
        let tiny = p.leave().unwrap();

        assert_eq!(&tiny.flash[..image.len()], &image[..]);
        assert!(tiny.flash[image.len()..].iter().all(|&b| b == 0xFF));
        assert_eq!(tiny.page_writes, 16);
        assert_eq!(tiny.page_erase_writes, 0);
        assert!(tiny.updi_disabled);
        assert!(!tiny.in_reset);
    }

    #[test]
    fn verify_catches_flash_that_did_not_erase() {
        let image: Vec<u8> = (0..200u32).map(|i| (i * 7 + 3) as u8).collect();
        let mut tiny = Tiny::new();
        tiny.flash.fill(0x42);
        tiny.ignores_chip_erase = true;

        let mut p = Programmer::connect(tiny).unwrap();
        p.enter_progmode().unwrap();
        p.erase_and_write_flash(&image).unwrap();
        let err = p.verify(map::FLASH, &image).unwrap_err();
        assert!(err.contains("verify failed"), "{err}");
    }

    #[test]
    fn refuses_an_image_bigger_than_flash_before_erasing() {
        let mut tiny = Tiny::new();
        tiny.flash.fill(0x42);
        let mut p = Programmer::connect(tiny).unwrap();
        p.enter_progmode().unwrap();
        let err = p
            .erase_and_write_flash(&vec![0; map::FLASH_SIZE + 1])
            .unwrap_err();
        assert!(err.contains("16384"), "{err}");
        let tiny = p.leave().unwrap();
        assert!(tiny.flash.iter().all(|&b| b == 0x42), "flash was erased");
    }

    #[test]
    fn skips_blank_flash_pages() {
        let mut image = vec![0xFF; 3 * map::FLASH_PAGE];
        image[2 * map::FLASH_PAGE] = 0x12;
        let mut p = Programmer::connect(Tiny::new()).unwrap();
        p.enter_progmode().unwrap();
        p.erase_and_write_flash(&image).unwrap();
        let tiny = p.leave().unwrap();
        assert_eq!(tiny.page_writes, 1);
        assert_eq!(tiny.page_erase_writes, 0);
    }

    #[test]
    fn writes_blank_eeprom_pages_over_eesave_contents() {
        let mut eeprom: Vec<u8> = (0..=255).collect();
        eeprom[3 * map::EEPROM_PAGE..4 * map::EEPROM_PAGE].fill(0xFF);
        let mut tiny = Tiny::new();
        tiny.eesave = true;
        tiny.eeprom.fill(0x42);

        let mut p = Programmer::connect(tiny).unwrap();
        p.enter_progmode().unwrap();
        p.erase_and_write_flash(&[0x12]).unwrap();
        p.write(map::EEPROM, map::EEPROM_PAGE, &eeprom).unwrap();
        p.verify(map::EEPROM, &eeprom).unwrap();
        let tiny = p.leave().unwrap();
        assert_eq!(tiny.page_erase_writes, 8);
    }

    #[test]
    fn eeprom_erase_write_leaves_bytes_it_was_not_given() {
        let eeprom = vec![0x11; 40];
        let mut tiny = Tiny::new();
        tiny.eeprom.fill(0x42);

        let mut p = Programmer::connect(tiny).unwrap();
        p.enter_progmode().unwrap();
        p.write(map::EEPROM, map::EEPROM_PAGE, &eeprom).unwrap();
        let tiny = p.leave().unwrap();
        assert_eq!(&tiny.eeprom[..40], &eeprom[..]);
        assert!(tiny.eeprom[40..].iter().all(|&b| b == 0x42));
    }

    #[test]
    fn writes_eeprom_over_eesave_contents_and_reads_fuses() {
        let eeprom: Vec<u8> = (0..=255).collect();
        let mut tiny = Tiny::new();
        tiny.eesave = true;
        tiny.eeprom.fill(0x42);

        let mut p = Programmer::connect(tiny).unwrap();
        p.enter_progmode().unwrap();
        p.erase_and_write_flash(&[0x12]).unwrap();
        p.write(map::EEPROM, map::EEPROM_PAGE, &eeprom).unwrap();
        p.verify(map::EEPROM, &eeprom).unwrap();
        let fuses = p.read(map::FUSES, map::FUSES_SIZE).unwrap();
        assert_eq!(fuses[2], 0x02); // OSCCFG says 20 MHz

        let tiny = p.leave().unwrap();
        assert_eq!(tiny.page_erase_writes, 8);
        assert_eq!(tiny.page_writes, 1);
    }

    #[test]
    fn reports_a_verify_mismatch() {
        let mut p = Programmer::connect(Tiny::new()).unwrap();
        p.enter_progmode().unwrap();
        let err = p.verify(map::FLASH, &[0xFF, 0x00]).unwrap_err();
        assert!(err.contains("0x8001"), "{err}");
    }

    /// Not in the ATDF, and the fake chip decodes most with the same names.
    /// DS40002204A: SYNCH §33.3.1.3, ACK figure 33-4, opcodes figure 33-8,
    /// addresses §33.4, bits §33.5.3 to §33.5.9, the reset signature §33.5.6,
    /// and the key, least significant byte first, table 33-5.
    #[test]
    fn updi_registers_and_instructions_are_the_datasheets() {
        assert_eq!(SYNC, 0x55, "SYNCH");
        assert_eq!(ACK, 0x40, "ACK");

        assert_eq!(
            [LDS, LD, STS, ST, LDCS, REPEAT, STCS, KEY],
            [0x00, 0x20, 0x40, 0x60, 0x80, 0xA0, 0xC0, 0xE0],
            "opcodes"
        );

        assert_eq!(CS_STATUSA, 0x00, "STATUSA");
        assert_eq!(CS_CTRLA, 0x02, "CTRLA");
        assert_eq!(CS_CTRLB, 0x03, "CTRLB");
        assert_eq!(ASI_KEY_STATUS, 0x07, "ASI_KEY_STATUS");
        assert_eq!(ASI_RESET_REQ, 0x08, "ASI_RESET_REQ");
        assert_eq!(ASI_SYS_STATUS, 0x0B, "ASI_SYS_STATUS");

        assert_eq!(CTRLA_IBDLY, 0x80, "CTRLA.IBDLY");
        assert_eq!(CTRLA_RSD, 0x08, "CTRLA.RSD");
        assert_eq!(CTRLB_CCDETDIS, 0x08, "CTRLB.CCDETDIS");
        assert_eq!(CTRLB_UPDIDIS, 0x04, "CTRLB.UPDIDIS");
        assert_eq!(KEY_STATUS_NVMPROG, 0x10, "ASI_KEY_STATUS.NVMPROG");
        assert_eq!(SYS_STATUS_NVMPROG, 0x08, "ASI_SYS_STATUS.NVMPROG");
        assert_eq!(SYS_STATUS_LOCKSTATUS, 0x01, "ASI_SYS_STATUS.LOCKSTATUS");
        assert_eq!(RESET_REQ_APPLY, 0x59, "the reset signature");
        assert_eq!(KEY_NVMPROG, b"NVMProg ", "the NVM programming key");
    }

    struct BadLine {
        fails: fn() -> io::Error,
        on_send: bool,
        breaks: usize,
    }

    impl Wire for &mut BadLine {
        fn send(&mut self, _: &[u8]) -> io::Result<()> {
            if self.on_send {
                Err((self.fails)())
            } else {
                Ok(())
            }
        }

        fn recv(&mut self, _: &mut [u8]) -> io::Result<()> {
            Err((self.fails)())
        }

        fn double_break(&mut self) -> io::Result<()> {
            self.breaks += 1;
            Ok(())
        }
    }

    #[test]
    fn a_chip_that_says_nothing_reads_as_no_reply_after_a_double_break() {
        let mut line = BadLine {
            fails: || io::ErrorKind::TimedOut.into(),
            on_send: false,
            breaks: 0,
        };
        let Err(err) = Programmer::connect(&mut line) else {
            panic!("connected to nothing");
        };
        assert_eq!(
            err,
            "no reply from the chip. Check the probe's contact, the pad order and that the \
             cell is out"
        );
        assert_eq!(line.breaks, 1, "one double break and a second try");
    }

    /// "Nothing echoed back" on the pads can mean a short, and "no reply"
    /// invites pressing harder.
    #[test]
    fn a_line_that_does_not_echo_is_not_reported_as_no_reply() {
        let mut line = BadLine {
            fails: || io::Error::other("nothing echoed back. Something's wrong"),
            on_send: true,
            breaks: 0,
        };
        let Err(err) = Programmer::connect(&mut line) else {
            panic!("connected to nothing");
        };
        assert_eq!(err, "serial: nothing echoed back. Something's wrong");
        assert!(!err.contains("no reply"), "{err}");
    }

    #[test]
    fn only_a_timeout_is_no_reply_and_any_other_error_keeps_its_words() {
        let err = io_error(io::Error::new(io::ErrorKind::TimedOut, "took too long"));
        assert!(err.starts_with("no reply from the chip."), "{err}");
        for kind in [
            io::ErrorKind::NotFound,
            io::ErrorKind::PermissionDenied,
            io::ErrorKind::BrokenPipe,
            io::ErrorKind::WouldBlock,
            io::ErrorKind::UnexpectedEof,
            io::ErrorKind::Other,
        ] {
            let err = io_error(io::Error::new(kind, "the adapter's gone"));
            assert_eq!(err, "serial: the adapter's gone", "{kind:?}");
        }
    }

    #[test]
    fn refuses_a_link_whose_statusa_reads_zero_after_a_double_break() {
        let mut tiny = Tiny::new();
        tiny.statusa = 0;
        let Err(err) = Programmer::connect(&mut tiny) else {
            panic!("connected");
        };
        assert_eq!(err, "the UPDI link came up but STATUSA reads 0");
        assert_eq!(tiny.breaks, 1, "one double break and a second try");
        assert!(!tiny.progmode, "the key went out");
        assert_eq!(tiny.resets, 0, "the chip was reset");
    }

    #[test]
    fn a_chip_that_refuses_the_key_is_not_reset_and_ends_with_updi_off() {
        let mut tiny = Tiny::new();
        tiny.ignores_key = true;

        let mut p = Programmer::connect(&mut tiny).unwrap();
        let err = p.enter_progmode().unwrap_err();
        assert_eq!(err, "the chip didn't accept the NVM programming key");
        assert_eq!(tiny.resets, 0, "the chip was reset without the key");

        let mut tiny = with_contents(tiny);
        let Err(err) = Programmer::connect_for_programming(&mut tiny) else {
            panic!("connected for programming");
        };
        assert_eq!(err, "the chip didn't accept the NVM programming key");
        assert_eq!(tiny.resets, 1, "only the reset that leaves");
        assert!(!tiny.in_reset, "the chip was left in reset");
        assert!(!tiny.progmode, "the chip was left in programming mode");
        assert!(tiny.updi_disabled, "UPDI was left on");
        assert!(tiny.flash.iter().all(|&b| b == 0x42), "flash changed");
        assert!(tiny.eeprom.iter().all(|&b| b == 0x24), "EEPROM changed");
    }

    #[test]
    fn only_a_chip_that_reads_unlocked_and_in_programming_mode_is_written_to() {
        for status in [
            0,
            SYS_STATUS_LOCKSTATUS,
            SYS_STATUS_LOCKSTATUS | SYS_STATUS_NVMPROG,
        ] {
            let mut tiny = Tiny::new();
            tiny.sys_status_after_reset = Some(status);
            let err = refused(tiny);
            assert!(
                err.starts_with("the chip didn't enter programming mode."),
                "{status:#04x}: {err}"
            );
            assert!(err.contains("which this tool won't do"), "{err}");
        }

        // 0x20 is RSTSYS.
        let mut tiny = Tiny::new();
        tiny.sys_status_after_reset = Some(0x20 | SYS_STATUS_NVMPROG);
        assert!(Programmer::connect_for_programming(tiny).is_ok());
    }

    #[test]
    fn a_locked_chip_is_given_the_whole_timeout_and_then_refused() {
        let mut tiny = Tiny::new();
        tiny.locked = true;
        let mut p = Programmer::connect(tiny).unwrap();
        let before = p.wire.elapsed;
        let err = p.enter_progmode().unwrap_err();
        assert!(err.contains("If it's locked, unlocking means erasing it"));
        let waited = p.wire.elapsed.checked_sub(before).unwrap();
        assert!(waited >= PROGMODE_TIMEOUT, "gave up after {waited:?}");
        assert_eq!(p.wire.page_writes + p.wire.page_erase_writes, 0);
    }

    #[test]
    fn already_in_programming_mode_but_locked_is_refused() {
        let mut p = Programmer::connect(Tiny::new()).unwrap();
        p.wire.progmode = true;
        p.wire.locked = true;
        let err = p.enter_progmode().unwrap_err();
        assert!(err.contains("unlocking means erasing it"), "{err}");
        assert_eq!(p.wire.resets, 1, "never sent the key and reset");
    }

    fn images() -> (Vec<u8>, Vec<u8>) {
        let flash = (0..1000u32).map(|i| (i * 7 + 3) as u8).collect();
        let eeprom = (0..=255).collect();
        (flash, eeprom)
    }

    fn write_both(tiny: Tiny) -> Tiny {
        let (flash, eeprom) = images();
        let mut p = Programmer::connect_for_programming(tiny).unwrap();
        p.erase_and_write_flash(&flash).unwrap();
        p.verify(map::FLASH, &flash).unwrap();
        p.write(map::EEPROM, map::EEPROM_PAGE, &eeprom).unwrap();
        p.verify(map::EEPROM, &eeprom).unwrap();
        p.leave().unwrap()
    }

    #[test]
    fn waits_out_every_erase_and_write_before_starting_the_next() {
        for (write, erase) in [(2, 4), (20, 45)] {
            for round_trip in [Duration::from_micros(100), Duration::from_millis(16)] {
                let mut tiny = with_contents(Tiny::new());
                tiny.write_time = Duration::from_millis(write);
                tiny.erase_time = Duration::from_millis(erase);
                tiny.round_trip = round_trip;
                let tiny = write_both(tiny);
                let case = format!("{write} and {erase} ms, {round_trip:?} a round trip");
                assert_eq!(tiny.lost_while_busy, 0, "{case}");
                assert_eq!(tiny.page_writes, 16, "{case}");
                assert_eq!(tiny.page_erase_writes, 8, "{case}");
                if round_trip < tiny.write_time {
                    assert!(tiny.busy_polls > 0, "{case}: never saw it busy");
                }
            }
        }
    }

    #[test]
    fn waits_for_eeprom_as_well_as_flash() {
        let (_, eeprom) = images();
        let mut tiny = with_contents(Tiny::new());
        tiny.erase_time = Duration::from_millis(30);
        let mut p = Programmer::connect_for_programming(tiny).unwrap();
        p.write(map::EEPROM, map::EEPROM_PAGE, &eeprom).unwrap();
        p.verify(map::EEPROM, &eeprom).unwrap();
        let tiny = p.leave().unwrap();
        assert_eq!(tiny.lost_while_busy, 0);
        assert!(tiny.busy_polls >= 8 * 25, "{} busy polls", tiny.busy_polls);
        assert!(tiny.flash.iter().all(|&b| b == 0x42), "flash changed");
    }

    /// DS40002204A §9.3.2.4 only says to check the busy flags first, so
    /// losing what comes early is the strict reading, not a fact.
    #[test]
    fn the_fake_chip_loses_what_is_sent_while_it_is_busy() {
        let mut tiny = Tiny::new();
        tiny.flash.fill(0x42);
        tiny.erase_time = Duration::from_millis(50);
        let mut p = Programmer::connect_for_programming(tiny).unwrap();
        p.nvm_command(NVM_CHIP_ERASE).unwrap();
        assert_ne!(p.lds(map::NVMCTRL_STATUS).unwrap() & NVM_FBUSY, 0);

        p.st_ptr(map::FLASH).unwrap();
        p.send(&[SYNC, ST | PTR_INC | DATA_8, 0x12]).unwrap();
        p.ack().unwrap();
        p.nvm_command(NVM_PAGE_WRITE).unwrap();
        assert_eq!(p.wire.lost_while_busy, 2, "the byte and the command");
        assert_eq!(p.wire.page_writes, 0);

        p.wait_nvm().unwrap();
        p.write_page(map::FLASH, &[0x12], NVM_PAGE_WRITE).unwrap();
        assert_eq!(p.read(map::FLASH, 2).unwrap(), [0x12, 0xFF]);
        assert_eq!(p.wire.lost_while_busy, 2);
    }

    #[test]
    fn a_chip_that_never_stops_being_busy_times_out_with_an_error() {
        let mut tiny = with_contents(Tiny::new());
        tiny.erase_time = Duration::MAX;
        tiny.round_trip = Duration::from_micros(100);
        let mut p = Programmer::connect_for_programming(tiny).unwrap();
        let before = p.wire.elapsed;
        let err = p.erase_and_write_flash(&[0x12; 200]).unwrap_err();
        assert_eq!(err, "timed out waiting for the NVM controller");
        let waited = p.wire.elapsed.checked_sub(before).unwrap();
        assert!(waited >= Duration::from_secs(1), "gave up after {waited:?}");

        let tiny = p.leave().unwrap();
        assert_eq!(tiny.page_writes, 0);
        assert_eq!(tiny.lost_while_busy, 0);
        assert!(tiny.updi_disabled, "UPDI was left on");
    }

    #[test]
    fn a_write_error_is_reported_and_stops_the_flash() {
        let (image, _) = images();
        let mut tiny = Tiny::new();
        tiny.write_error_at = Some(3);
        let mut p = Programmer::connect_for_programming(tiny).unwrap();
        let err = p.erase_and_write_flash(&image).unwrap_err();
        assert_eq!(err, "the NVM controller reported a write error");

        let tiny = p.leave().unwrap();
        assert_eq!(tiny.page_writes, 3);
        let written = 3 * map::FLASH_PAGE;
        assert_eq!(&tiny.flash[..written], &image[..written]);
        assert!(tiny.flash[written..].iter().all(|&b| b == 0xFF));
        assert_eq!(tiny.lost_while_busy, 0);
    }

    #[test]
    fn a_write_error_in_eeprom_is_reported_too() {
        let (_, eeprom) = images();
        let mut tiny = Tiny::new();
        tiny.write_error_at = Some(0);
        let mut p = Programmer::connect_for_programming(tiny).unwrap();
        let err = p.write(map::EEPROM, map::EEPROM_PAGE, &eeprom).unwrap_err();
        assert_eq!(err, "the NVM controller reported a write error");
        assert_eq!(p.wire.page_erase_writes, 0);
    }

    /// Written from the same datasheet as the programmer, so it catches
    /// slips, not misreadings.
    pub(crate) mod fake {
        use super::super::*;
        use std::collections::VecDeque;

        const FLASH_END: u16 = map::FLASH + (map::FLASH_SIZE - 1) as u16;
        const EEPROM_END: u16 = map::EEPROM + (map::EEPROM_SIZE - 1) as u16;
        const SIGNATURE_END: u16 = map::SIGNATURE + (map::ATTINY1616.len() - 1) as u16;
        const SERIAL_END: u16 = map::SERIAL + (map::SERIAL_SIZE - 1) as u16;
        const OSCCFG: u16 = map::FUSES + 2;

        fn region(addr: u16) -> (u16, usize) {
            if addr >= map::FLASH {
                (map::FLASH, map::FLASH_PAGE)
            } else {
                (map::EEPROM, map::EEPROM_PAGE)
            }
        }

        enum Expect {
            Sync,
            Opcode,
            CsValue(u8),
            Key(Vec<u8>),
            LdsAddress(Vec<u8>),
            StsAddress(Vec<u8>),
            StsData(u16),
            PtrAddress(Vec<u8>),
            RepeatCount,
            StIncData(usize),
        }

        pub struct Tiny {
            pub flash: Vec<u8>,
            pub eeprom: Vec<u8>,
            pub signature: [u8; 3],
            /// Made up, since nobody has read a real chip's.
            pub serial: [u8; map::SERIAL_SIZE],
            pub locked: bool,
            pub asleep: bool,
            pub breaks: usize,
            pub sent: usize,
            pub updi_disabled: bool,
            pub in_reset: bool,
            pub resets: usize,
            pub progmode: bool,
            pub start_up: Duration,
            /// The fake clock moves on by this much per `recv`.
            pub round_trip: Duration,
            pub elapsed: Duration,
            clock_start: Instant,
            started_at: Duration,
            pub eesave: bool,
            pub ignores_chip_erase: bool,
            pub page_writes: usize,
            pub page_erase_writes: usize,
            /// The UPDI revision is in its high nibble, so it never reads 0.
            pub statusa: u8,
            /// Nothing says why a real chip wouldn't take the key.
            pub ignores_key: bool,
            /// Nothing says a real chip reads any of these.
            pub sys_status_after_reset: Option<u8>,
            pub write_time: Duration,
            pub erase_time: Duration,
            pub busy_polls: usize,
            /// The datasheet only rejects writes after a power-on reset
            /// (DS40002204A §9.3.2.5). Dropping these is the strict reading.
            pub lost_while_busy: usize,
            /// Nothing models what makes a real controller report WRERROR.
            pub write_error_at: Option<usize>,
            commits: usize,
            write_error: bool,
            busy: u8,
            busy_until: Duration,
            expect: Expect,
            replies: VecDeque<u8>,
            ctrla: u8,
            key_status: u8,
            ptr: u16,
            repeat: usize,
            page_buffer: Vec<u8>,
            page_loaded: Vec<bool>,
            page_addr: Option<u16>,
        }

        impl Tiny {
            pub fn new() -> Self {
                Self {
                    flash: vec![0xFF; map::FLASH_SIZE],
                    eeprom: vec![0xFF; map::EEPROM_SIZE],
                    signature: map::ATTINY1616,
                    serial: [0x51, 0x3A, 0x07, 0xC4, 0x19, 0xE2, 0x6D, 0x80, 0x2B, 0xF5],
                    locked: false,
                    asleep: false,
                    breaks: 0,
                    sent: 0,
                    updi_disabled: false,
                    in_reset: false,
                    resets: 0,
                    progmode: false,
                    start_up: Duration::ZERO,
                    // A full-speed USB frame.
                    round_trip: Duration::from_millis(1),
                    elapsed: Duration::ZERO,
                    clock_start: Instant::now(),
                    started_at: Duration::ZERO,
                    eesave: false,
                    ignores_chip_erase: false,
                    page_writes: 0,
                    page_erase_writes: 0,
                    statusa: 0x30, // UPDI revision 3
                    ignores_key: false,
                    sys_status_after_reset: None,
                    write_time: Duration::from_millis(2),
                    erase_time: Duration::from_millis(4),
                    busy_polls: 0,
                    lost_while_busy: 0,
                    write_error_at: None,
                    commits: 0,
                    write_error: false,
                    busy: 0,
                    busy_until: Duration::ZERO,
                    expect: Expect::Sync,
                    replies: VecDeque::new(),
                    ctrla: 0,
                    key_status: 0,
                    ptr: 0,
                    repeat: 1,
                    page_buffer: vec![0xFF; map::FLASH_PAGE],
                    page_loaded: vec![false; map::FLASH_PAGE],
                    page_addr: None,
                }
            }

            fn byte(&mut self, b: u8) {
                self.expect = match std::mem::replace(&mut self.expect, Expect::Sync) {
                    Expect::Sync => {
                        assert_eq!(b, SYNC, "every instruction starts with SYNC");
                        Expect::Opcode
                    }
                    Expect::Opcode => self.opcode(b),
                    Expect::CsValue(reg) => {
                        self.stcs(reg, b);
                        Expect::Sync
                    }
                    Expect::Key(mut key) => {
                        key.push(b);
                        if key.len() < 8 {
                            Expect::Key(key)
                        } else {
                            key.reverse();
                            if key == KEY_NVMPROG && !self.ignores_key {
                                self.key_status |= KEY_STATUS_NVMPROG;
                            }
                            Expect::Sync
                        }
                    }
                    Expect::LdsAddress(mut a) => {
                        a.push(b);
                        if a.len() < 2 {
                            Expect::LdsAddress(a)
                        } else {
                            let v = self.load(u16::from_le_bytes([a[0], a[1]]));
                            self.replies.push_back(v);
                            Expect::Sync
                        }
                    }
                    Expect::StsAddress(mut a) => {
                        a.push(b);
                        if a.len() < 2 {
                            Expect::StsAddress(a)
                        } else {
                            self.ack();
                            Expect::StsData(u16::from_le_bytes([a[0], a[1]]))
                        }
                    }
                    Expect::StsData(addr) => {
                        self.store(addr, b);
                        self.ack();
                        Expect::Sync
                    }
                    Expect::PtrAddress(mut a) => {
                        a.push(b);
                        if a.len() < 2 {
                            Expect::PtrAddress(a)
                        } else {
                            self.ptr = u16::from_le_bytes([a[0], a[1]]);
                            self.ack();
                            Expect::Sync
                        }
                    }
                    Expect::RepeatCount => {
                        self.repeat = usize::from(b) + 1;
                        Expect::Sync
                    }
                    Expect::StIncData(left) => {
                        self.store(self.ptr, b);
                        self.ptr += 1;
                        self.ack();
                        if left > 1 {
                            Expect::StIncData(left - 1)
                        } else {
                            Expect::Sync
                        }
                    }
                };
            }

            fn opcode(&mut self, op: u8) -> Expect {
                let repeat = std::mem::replace(&mut self.repeat, 1);
                match op {
                    _ if op & 0xF0 == LDCS => {
                        let v = self.ldcs(op & 0x0F);
                        self.replies.push_back(v);
                        Expect::Sync
                    }
                    _ if op & 0xF0 == STCS => Expect::CsValue(op & 0x0F),
                    0xE0 => Expect::Key(Vec::new()),
                    0xE5 => {
                        // What another real ATtiny1616 reported. The TS26
                        // has the same part, but nobody's read one yet.
                        self.replies.extend(b"tinyAVR P:0D:0-3");
                        Expect::Sync
                    }
                    0x04 => Expect::LdsAddress(Vec::new()),
                    0x44 => Expect::StsAddress(Vec::new()),
                    0x69 => Expect::PtrAddress(Vec::new()),
                    0xA0 => Expect::RepeatCount,
                    0x24 => {
                        for _ in 0..repeat {
                            let v = self.load(self.ptr);
                            self.replies.push_back(v);
                            self.ptr += 1;
                        }
                        Expect::Sync
                    }
                    0x64 => Expect::StIncData(repeat),
                    _ => panic!("unexpected opcode 0x{op:02X}"),
                }
            }

            fn ack(&mut self) {
                if self.ctrla & CTRLA_RSD == 0 {
                    self.replies.push_back(ACK);
                }
            }

            fn ldcs(&self, reg: u8) -> u8 {
                match (reg, self.sys_status_after_reset) {
                    (ASI_SYS_STATUS, Some(status)) if self.resets > 0 => return status,
                    _ => {}
                }
                match reg {
                    CS_STATUSA => self.statusa,
                    ASI_KEY_STATUS => self.key_status,
                    ASI_SYS_STATUS if self.elapsed < self.started_at => SYS_STATUS_LOCKSTATUS,
                    ASI_SYS_STATUS => {
                        let lock = if self.locked {
                            SYS_STATUS_LOCKSTATUS
                        } else {
                            0
                        };
                        let prog = if self.progmode { SYS_STATUS_NVMPROG } else { 0 };
                        lock | prog
                    }
                    _ => 0,
                }
            }

            fn stcs(&mut self, reg: u8, value: u8) {
                match reg {
                    CS_CTRLA => self.ctrla = value,
                    CS_CTRLB => {
                        self.updi_disabled = value & CTRLB_UPDIDIS != 0;
                        if self.updi_disabled {
                            // DS §33.3.2.2.2.
                            self.progmode = false;
                            self.key_status = 0;
                        }
                    }
                    ASI_RESET_REQ if value == RESET_REQ_APPLY => self.in_reset = true,
                    ASI_RESET_REQ => {
                        if self.in_reset {
                            self.resets += 1;
                        }
                        self.in_reset = false;
                        self.started_at = self.elapsed + self.start_up;
                        self.progmode = self.key_status & KEY_STATUS_NVMPROG != 0 && !self.locked;
                        self.key_status = 0;
                    }
                    _ => {}
                }
            }

            fn is_busy(&self) -> bool {
                self.elapsed < self.busy_until
            }

            fn nvm_status(&mut self) -> u8 {
                let error = if self.write_error { NVM_WRERROR } else { 0 };
                if self.is_busy() {
                    self.busy_polls += 1;
                    self.busy | error
                } else {
                    error
                }
            }

            fn load(&mut self, addr: u16) -> u8 {
                match addr {
                    map::NVMCTRL_STATUS => self.nvm_status(),
                    map::FLASH..=FLASH_END => self.flash[usize::from(addr - map::FLASH)],
                    map::EEPROM..=EEPROM_END => self.eeprom[usize::from(addr - map::EEPROM)],
                    map::SIGNATURE..=SIGNATURE_END => {
                        self.signature[usize::from(addr - map::SIGNATURE)]
                    }
                    map::SERIAL..=SERIAL_END => self.serial[usize::from(addr - map::SERIAL)],
                    OSCCFG => 0x02,
                    _ => 0,
                }
            }

            fn store(&mut self, addr: u16, value: u8) {
                assert!(
                    self.progmode,
                    "store to 0x{addr:04X} outside programming mode"
                );
                match addr {
                    map::NVMCTRL_CTRLA => self.nvm(value),
                    map::FLASH..=FLASH_END | map::EEPROM..=EEPROM_END if self.is_busy() => {
                        self.lost_while_busy += 1;
                    }
                    map::FLASH..=FLASH_END | map::EEPROM..=EEPROM_END => {
                        let (_, page) = region(addr);
                        self.page_addr = Some(addr - addr % page as u16);
                        self.page_buffer[usize::from(addr) % page] = value;
                        self.page_loaded[usize::from(addr) % page] = true;
                    }
                    _ => panic!("store to 0x{addr:04X}"),
                }
            }

            fn start(&mut self, flags: u8, time: Duration) {
                self.busy = flags;
                self.busy_until = self.elapsed.saturating_add(time);
            }

            fn nvm(&mut self, command: u8) {
                if self.is_busy() {
                    self.lost_while_busy += 1;
                    return;
                }
                // WRERROR "is valid for the last operation" (DS40002204A
                // §9.5.3). Which commands clear it on a real chip is
                // unconfirmed.
                self.write_error = false;
                match command {
                    NVM_PAGE_BUFFER_CLEAR => {
                        self.page_buffer.fill(0xFF);
                        self.page_loaded.fill(false);
                        self.page_addr = None;
                    }
                    NVM_PAGE_WRITE | NVM_PAGE_ERASE_WRITE => {
                        let addr = self.page_addr.take().expect("a page to write");
                        let (base, page) = region(addr);
                        let commit = self.commits;
                        self.commits += 1;
                        if self.write_error_at == Some(commit) {
                            self.write_error = true;
                            return;
                        }
                        let flag = if base == map::FLASH {
                            NVM_FBUSY
                        } else {
                            NVM_EEBUSY
                        };
                        let time = if command == NVM_PAGE_WRITE {
                            self.write_time
                        } else {
                            self.erase_time
                        };
                        self.start(flag, time);
                        let mem = if base == map::FLASH {
                            &mut self.flash
                        } else {
                            &mut self.eeprom
                        };
                        let start = usize::from(addr - base);
                        let cells = mem[start..start + page].iter_mut();
                        let bytes = self.page_buffer.iter().zip(&self.page_loaded);
                        if command == NVM_PAGE_WRITE {
                            // A write can only clear bits; only an erase sets them.
                            for (cell, (&b, _)) in cells.zip(bytes) {
                                *cell &= b;
                            }
                            self.page_writes += 1;
                        } else {
                            // Flash erases the whole page. EEPROM erases only
                            // the bytes loaded into the page buffer (DS40001893A,
                            // 9.3.1.2, byte granularity on erase-write).
                            for (cell, (&b, &loaded)) in cells.zip(bytes) {
                                if base == map::FLASH || loaded {
                                    *cell = b;
                                }
                            }
                            self.page_erase_writes += 1;
                        }
                        self.page_buffer.fill(0xFF);
                        self.page_loaded.fill(false);
                    }
                    NVM_CHIP_ERASE if self.ignores_chip_erase => {}
                    NVM_CHIP_ERASE => {
                        // Which flags a real chip raises is unconfirmed.
                        self.start(NVM_FBUSY | NVM_EEBUSY, self.erase_time);
                        self.flash.fill(0xFF);
                        if !self.eesave {
                            self.eeprom.fill(0xFF);
                        }
                    }
                    _ => panic!("unexpected NVM command 0x{command:02X}"),
                }
            }
        }

        impl Wire for Tiny {
            fn send(&mut self, bytes: &[u8]) -> io::Result<()> {
                self.sent += bytes.len();
                for &b in bytes {
                    self.byte(b);
                }
                Ok(())
            }

            fn recv(&mut self, buf: &mut [u8]) -> io::Result<()> {
                self.elapsed += self.round_trip;
                if self.asleep {
                    self.replies.clear();
                    self.expect = Expect::Sync;
                    return Err(io::ErrorKind::TimedOut.into());
                }
                for slot in buf {
                    *slot = self
                        .replies
                        .pop_front()
                        .ok_or_else(|| io::Error::from(io::ErrorKind::TimedOut))?;
                }
                Ok(())
            }

            fn double_break(&mut self) -> io::Result<()> {
                self.breaks += 1;
                self.asleep = false;
                self.expect = Expect::Sync;
                self.replies.clear();
                Ok(())
            }

            fn now(&self) -> Instant {
                self.clock_start + self.elapsed
            }
        }

        impl Wire for &mut Tiny {
            fn send(&mut self, bytes: &[u8]) -> io::Result<()> {
                (**self).send(bytes)
            }

            fn recv(&mut self, buf: &mut [u8]) -> io::Result<()> {
                (**self).recv(buf)
            }

            fn double_break(&mut self) -> io::Result<()> {
                (**self).double_break()
            }

            fn now(&self) -> Instant {
                (**self).now()
            }
        }
    }
}
