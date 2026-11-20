//! Not firmware. Runs hw.rs's power-down guard in `prepare_sleep`, which the
//! firmware never asks it to, for `cargo xtask hex` to check on the simulator.
//! Never flashed.

#![cfg_attr(target_arch = "avr", no_std, no_main)]
#![cfg_attr(target_arch = "avr", feature(asm_experimental_arch))]
#![cfg_attr(target_arch = "avr", feature(abi_avr_interrupt))]

#[cfg(target_arch = "avr")]
use ts26::regs;

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

    /// `sim::guards` counts on this.
    const LIT_TICKS: u8 = 4;

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
        hw::set_output(Output {
            enable: false,
            dsm: 1000,
        });
        if !hw::wait_for_tick(true) {
            hw::halt();
        }
        hw::set_output(Output::OFF);
        if !hw::wait_for_tick(true) {
            hw::halt();
        }
        guards_done()
    }

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
