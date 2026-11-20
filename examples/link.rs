//! Not firmware. One static of each kind, for `cargo xtask hex` to check link.x
//! and src/avr/rt.rs against, since the firmware has none. Never flashed.

#![cfg_attr(target_arch = "avr", no_std, no_main)]
#![cfg_attr(target_arch = "avr", feature(asm_experimental_arch))]
#![cfg_attr(target_arch = "avr", feature(abi_avr_interrupt))]

#[cfg(target_arch = "avr")]
use ts26::regs;
#[cfg(target_arch = "avr")]
#[path = "../src/avr/rt.rs"]
mod rt;

/// rt.rs's vector table jumps to these by name. Nothing enables an interrupt.
#[cfg(target_arch = "avr")]
#[expect(
    clippy::missing_const_for_fn,
    reason = "interrupt handlers and the runtime's halt, reached from the vector table, never \
              evaluated at compile time"
)]
mod hw {
    pub extern "avr-interrupt" fn porta_port() {}
    pub extern "avr-interrupt" fn rtc_pit() {}
    pub extern "avr-interrupt" fn tca0_ovf() {}

    #[allow(clippy::empty_loop)]
    pub fn halt() -> ! {
        loop {}
    }
}

#[cfg(target_arch = "avr")]
mod probe {
    use core::hint::black_box;
    use core::sync::atomic::{AtomicU8, Ordering::Relaxed};

    // Odd lengths, so .data and .bss need link.x's ALIGN(2) padding.

    #[allow(unsafe_code)]
    #[unsafe(no_mangle)]
    static LINK_PROBE_TABLE: [u8; 3] = [0x12, 0x34, 0x56];

    #[allow(unsafe_code)]
    #[unsafe(no_mangle)]
    static LINK_PROBE_DATA: [AtomicU8; 3] = [
        AtomicU8::new(0x9A),
        AtomicU8::new(0xBC),
        AtomicU8::new(0xDE),
    ];

    #[allow(unsafe_code)]
    #[unsafe(no_mangle)]
    static LINK_PROBE_COPY: [AtomicU8; 7] = [const { AtomicU8::new(0) }; 7];

    #[allow(unsafe_code)]
    #[unsafe(no_mangle)]
    extern "C" fn main() -> ! {
        // So the reads go through the addresses link.x gave the statics.
        let table = black_box(&LINK_PROBE_TABLE);
        let data = black_box(&LINK_PROBE_DATA);
        let copy = black_box(&LINK_PROBE_COPY);
        let (from_table, from_data) = copy.split_at(table.len());
        for (to, &b) in from_table.iter().zip(table) {
            to.store(b, Relaxed);
        }
        for (to, from) in from_data.iter().zip(data) {
            to.store(from.load(Relaxed), Relaxed);
        }
        link_probe_done()
    }

    #[allow(unsafe_code, clippy::empty_loop)]
    #[unsafe(no_mangle)]
    #[inline(never)]
    #[expect(
        clippy::missing_const_for_fn,
        reason = "a named stopping point the simulator looks for, never evaluated at compile time"
    )]
    extern "C" fn link_probe_done() -> ! {
        loop {}
    }

    #[panic_handler]
    #[expect(
        clippy::missing_const_for_fn,
        reason = "the panic handler, never evaluated at compile time"
    )]
    fn panic(_: &core::panic::PanicInfo) -> ! {
        loop {}
    }
}

#[cfg(not(target_arch = "avr"))]
fn main() {
    eprintln!("this is a linker check for the ATtiny1616; `cargo xtask hex` runs it");
    std::process::exit(1);
}
