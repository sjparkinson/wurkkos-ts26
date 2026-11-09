//! A serial UPDI adapter on a Unix serial port, set up with `stty`.
//!
//! Configuring a port properly means termios, which means libc or a serial
//! crate. `stty` is on every Mac and Linux box already and does the same
//! job: we open the port, hand the open file to `stty` as its stdin, and the
//! settings stick for as long as we hold it open. Windows isn't supported.

use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::process::{Command, Stdio};

use crate::updi::Wire;

/// What pymcuprog and avrdude both default to, and comfortably inside what
/// UPDI manages on the chip's 4 MHz reset clock.
const BAUD: u32 = 115_200;

/// Slow enough that one 0x00 frame holds the line low for longer than a
/// UPDI break needs.
const BREAK_BAUD: u32 = 300;

/// How long a read waits for silence before giving up, in the tenths of a
/// second that `stty time` counts in. Long enough for any reply.
const READ_TIMEOUT: u8 = 10;

/// The same while draining: 100 ms of quiet means nothing else is coming.
const DRAIN_TIMEOUT: u8 = 1;

/// Reads of junk before deciding the line will never go quiet.
const DRAIN_READS: usize = 64;

/// Silence has two causes that look the same from here, so it names both.
const NOTHING_ECHOED: &str = "nothing echoed back. A serial UPDI adapter joins TX and RX, so \
     every byte sent should come straight back. Either this adapter doesn't, or something's \
     pulling the UPDI line down: the wrong pad, a short, or a damaged chip. Stop and check \
     the wiring with a meter before trying again";

/// What a line held low sounds like, when the adapter reports it at all.
const LINE_HELD_LOW: &str = "every byte came back as 0, so something's pulling the UPDI line \
     down: the wrong pad, a short, or a damaged chip. Stop and check the wiring with a meter \
     before trying again";

pub struct Port {
    file: File,
}

/// Compares what came back with what was sent. A byte sent as 0 proves nothing
/// about the line, so only a send with something else in it can say it's low.
fn check_echo(sent: &[u8], echo: &[u8]) -> io::Result<()> {
    if echo == sent {
        return Ok(());
    }
    if echo.iter().all(|&b| b == 0) {
        return Err(io::Error::other(LINE_HELD_LOW));
    }
    Err(io::Error::other(
        "the echo didn't match what was sent, so something else is on the line",
    ))
}

/// Reads and throws away whatever `port` has until a read comes back empty,
/// which is a read that timed out with nothing to give.
fn read_until_quiet(port: &mut impl Read) -> io::Result<()> {
    let mut junk = [0; 64];
    for _ in 0..DRAIN_READS {
        if port.read(&mut junk)? == 0 {
            return Ok(());
        }
    }
    Err(io::Error::other(
        "the line never went quiet, so something is driving it",
    ))
}

impl Port {
    pub fn open(path: &str) -> Result<Self, String> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
            .map_err(|e| format!("{path}: {e}"))?;
        let mut port = Self { file };
        port.drain().map_err(|e| format!("{path}: {e}"))?;
        Ok(port)
    }

    fn configure(&self, baud: u32, timeout: u8) -> io::Result<()> {
        let status = Command::new("stty")
            // `raw` first: on macOS it's cfmakeraw, which also resets the
            // character size and parity that the later flags set.
            .args(["raw", "-echo", "clocal", "-crtscts", "-ixon", "-ixoff"])
            // UPDI frames are 8 data bits, even parity and two stop bits.
            .args(["cs8", "parenb", "-parodd", "cstopb"])
            // Reads return after `timeout` of silence rather than blocking
            // forever.
            .args(["min", "0", "time", &timeout.to_string()])
            .arg(baud.to_string())
            .stdin(Stdio::from(self.file.try_clone()?))
            .status()?;
        if status.success() {
            Ok(())
        } else {
            Err(io::Error::other(format!("stty failed ({status})")))
        }
    }

    /// Sets the port up at [`BAUD`] and throws away anything already waiting
    /// in the adapter, so the next echo starts clean. `send` checks every
    /// echo byte for byte, so one stale byte (noise from the probe landing,
    /// or a run stopped part way) would otherwise fail the first instruction
    /// with a misleading "echo didn't match". avrdude drains on open and after
    /// a double break for the same reason: adapters were seen echoing a break
    /// late enough to shift every reply after it by a byte (avrdude issue
    /// #874).
    fn drain(&mut self) -> io::Result<()> {
        self.configure(BAUD, DRAIN_TIMEOUT)?;
        read_until_quiet(&mut self.file)?;
        self.configure(BAUD, READ_TIMEOUT)
    }

    fn read_full(&mut self, buf: &mut [u8]) -> io::Result<()> {
        let mut filled = 0;
        while filled < buf.len() {
            match self.file.read(&mut buf[filled..])? {
                0 => return Err(io::ErrorKind::TimedOut.into()),
                n => filled += n,
            }
        }
        Ok(())
    }
}

impl Wire for Port {
    fn send(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.file.write_all(bytes)?;
        let mut echo = vec![0; bytes.len()];
        self.read_full(&mut echo).map_err(|e| match e.kind() {
            io::ErrorKind::TimedOut => io::Error::other(NOTHING_ECHOED),
            _ => e,
        })?;
        check_echo(bytes, &echo)
    }

    fn recv(&mut self, buf: &mut [u8]) -> io::Result<()> {
        self.read_full(buf)
    }

    fn double_break(&mut self) -> io::Result<()> {
        self.configure(BREAK_BAUD, READ_TIMEOUT)?;
        self.file.write_all(&[0, 0])?;
        // The break mangles its own echo, so it may come back as two bytes,
        // fewer, or late. Wait for what arrives in time, then drain the rest.
        let mut echo = [0; 2];
        let _ = self.read_full(&mut echo);
        self.drain()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_clean_echo_passes() {
        assert!(check_echo(&[0x55, 0xC0], &[0x55, 0xC0]).is_ok());
        assert!(check_echo(&[0, 0], &[0, 0]).is_ok());
    }

    #[test]
    fn an_echo_of_zeros_means_the_line_is_held_low() {
        let err = check_echo(&[0x55, 0xC0], &[0, 0]).unwrap_err();
        assert!(
            err.to_string().contains("pulling the UPDI line down"),
            "{err}"
        );
    }

    #[test]
    fn any_other_echo_means_something_else_is_on_the_line() {
        let err = check_echo(&[0x55, 0xC0], &[0x55, 0x00]).unwrap_err();
        assert!(err.to_string().contains("something else"), "{err}");
    }

    #[test]
    fn draining_reads_what_is_waiting_and_stops_at_the_first_quiet_read() {
        // Three reads' worth, the last one short, and then nothing.
        let mut waiting: &[u8] = &[0xA5; 150];
        read_until_quiet(&mut waiting).unwrap();
        assert!(waiting.is_empty());
    }

    #[test]
    fn draining_gives_up_on_a_line_that_never_goes_quiet() {
        let mut noisy = io::repeat(0);
        let err = read_until_quiet(&mut noisy).unwrap_err();
        assert!(err.to_string().contains("never went quiet"), "{err}");
    }

    #[test]
    fn silence_keeps_its_old_words_and_names_a_pulled_down_line() {
        assert!(NOTHING_ECHOED.starts_with("nothing echoed back"));
        assert!(NOTHING_ECHOED.contains("adapter"));
        assert!(NOTHING_ECHOED.contains("pulling the UPDI line down"));
    }
}
