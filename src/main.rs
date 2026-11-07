//! The firmware: sample the switch, step the UI, set the output, 64 times a
//! second.
//!
//! On the host this builds to a stub, so `cargo test` and `cargo clippy`
//! work across the workspace. `cargo xtask hex` builds the real thing.

#![cfg_attr(target_arch = "avr", no_std, no_main)]
#![cfg_attr(target_arch = "avr", feature(asm_experimental_arch))]
#![cfg_attr(target_arch = "avr", feature(abi_avr_interrupt))]

// The ADC and the self-tests' reads wait on src/sensors.rs and src/selftest.rs.
#[cfg(target_arch = "avr")]
#[allow(dead_code)]
mod hw;
// hw.rs and rt.rs need some of regs.rs's constants, not all of them.
#[cfg(target_arch = "avr")]
#[allow(dead_code)]
mod regs;
#[cfg(target_arch = "avr")]
mod rt;

// src/rt.rs calls `main` by name once memory is set up.
#[cfg(target_arch = "avr")]
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
extern "C" fn main() -> ! {
    use ts26::torch::{Changes, Torch};

    hw::init();

    let mut torch = Torch::new();
    let mut changes = Changes::new();

    loop {
        hw::wait_for_tick(torch.resting());
        torch.tick(hw::switch_pressed(), None);
        if let Some(output) = changes.update(torch.output()) {
            hw::set_output(output);
        }
        hw::set_aux(torch.aux());
    }
}

#[cfg(target_arch = "avr")]
#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    hw::set_output(ts26::ramp::Output::OFF);
    loop {}
}

#[cfg(not(target_arch = "avr"))]
fn main() {
    eprintln!("this is firmware for the ATtiny1616; build it with `cargo xtask hex`");
    std::process::exit(1);
}
