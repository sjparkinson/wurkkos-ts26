//! Not firmware, but src/avr/hw.rs driven in a way the torch firmware never
//! drives it, for `cargo xtask` to check hw.rs's own guards with. The library
//! never says it's resting with the light on, so `main` never asks
//! `hw::wait_for_tick` for power-down with PB2 high, and the guard in
//! `prepare_sleep` that idles instead is never run by the firmware.
//!
//! `main` lights the output, waits for a few ticks asking for power-down each
//! time, turns the output off, waits for one more asking the same, and stops
//! in `guards_done`. xtask runs it on the simulator from reset and checks it
//! only powered down that last time (`sim::guards`). Without the guard, the
//! simulator's own rule against power-down with PB2 high refuses it first.
//! `cargo xtask hex` builds this alongside the firmware; it's never flashed.

#![cfg_attr(target_arch = "avr", no_std, no_main)]
#![cfg_attr(target_arch = "avr", feature(asm_experimental_arch))]
#![cfg_attr(target_arch = "avr", feature(abi_avr_interrupt))]

// hw.rs and rt.rs reach the library's registers as `crate::regs`.
#[cfg(target_arch = "avr")]
use ts26::regs;

// Most of hw.rs goes unused here.
#[cfg(target_arch = "avr")]
#[allow(dead_code)]
#[path = "../src/avr/hw.rs"]
mod hw;
#[cfg(target_arch = "avr")]
#[path = "../src/avr/rt.rs"]
mod rt;

#[cfg(target_arch = "avr")]
mod guards {
    use crate::hw;
    use ts26::ramp::Output;

    /// Lit passes asking for power-down. `sim::guards` counts on this.
    const LIT_TICKS: u8 = 4;

    // src/avr/rt.rs calls `main` by name once memory is set up.
    #[allow(unsafe_code)]
    #[unsafe(no_mangle)]
    extern "C" fn main() -> ! {
        hw::hold_enable_low();
        hw::init();
        hw::set_output(Output {
            enable: true,
            dsm: 1000,
        });
        for _ in 0..LIT_TICKS {
            if !hw::wait_for_tick(true) {
                hw::halt();
            }
        }
        hw::set_output(Output::OFF);
        if !hw::wait_for_tick(true) {
            hw::halt();
        }
        guards_done()
    }

    /// Where the probe stops. Nothing runs it on a real chip, so spinning
    /// costs nothing.
    #[allow(unsafe_code, clippy::empty_loop)]
    #[unsafe(no_mangle)]
    #[inline(never)]
    #[expect(
        clippy::missing_const_for_fn,
        reason = "a named stopping point the simulator looks for, never evaluated at compile time"
    )]
    extern "C" fn guards_done() -> ! {
        loop {}
    }

    #[panic_handler]
    fn panic(_: &core::panic::PanicInfo) -> ! {
        hw::halt()
    }
}

#[cfg(not(target_arch = "avr"))]
fn main() {
    eprintln!("this is a check of hw.rs for the ATtiny1616; `cargo xtask hex` runs it");
    std::process::exit(1);
}
