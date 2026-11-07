//! The firmware: sample the switch, step the UI, set the output, 64 times a
//! second.
//!
//! On the host this builds to a stub, so `cargo test` and `cargo clippy`
//! work across the workspace. `cargo xtask hex` builds the real thing.

#![cfg_attr(target_arch = "avr", no_std, no_main)]
#![cfg_attr(target_arch = "avr", feature(asm_experimental_arch))]
#![cfg_attr(target_arch = "avr", feature(abi_avr_interrupt))]

#[cfg(target_arch = "avr")]
mod hw;
// The library owns src/regs.rs, since its self-tests check fuse fields with
// the same masks; hw.rs and rt.rs reach it as `crate::regs`.
#[cfg(target_arch = "avr")]
use ts26::regs;
#[cfg(target_arch = "avr")]
mod rt;

// src/rt.rs calls `main` by name once memory is set up.
#[cfg(target_arch = "avr")]
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
extern "C" fn main() -> ! {
    use ts26::selftest::{self, Faults};
    use ts26::torch::{Changes, Torch};
    use ts26::watchdog::Feeder;

    // PB2 floats from reset until it's an output, so that's first of all.
    hw::hold_enable_low();
    // What this reset means comes next, so the flags are clear before
    // anything can reset the chip again. The checks come before anything's
    // turned on. A failure doesn't stop the torch, it quietly caps the light
    // (src/selftest.rs).
    let faults = Faults {
        hang: selftest::hung(hw::take_reset_flags()),
        flash: !hw::flash_ok(),
        fuses: !hw::fuses().ok(),
    };

    hw::init();
    // After the PIT's running, so the first tick comes before the WDT's first
    // timeout.
    hw::start_watchdog();

    let mut torch = Torch::new(hw::temperature_calibration(), faults);
    let mut changes = Changes::new();
    let mut aux_changes = Changes::new();
    // Beside the torch rather than in it, which costs 150 bytes less flash.
    let mut feeder = Feeder::new();

    loop {
        // Power-down or idle, by `Feeder::power_down`'s rule, which the
        // simulator holds this loop to.
        hw::wait_for_tick(feeder.power_down(torch.resting()));
        torch.tick(hw::switch_pressed(), hw::adc_result());
        if let Some(convert) = torch.convert() {
            hw::adc_start(convert);
        }
        if let Some(output) = changes.update(torch.output()) {
            hw::set_output(output);
        }
        if let Some(aux) = aux_changes.update(torch.aux()) {
            hw::set_aux(aux);
        }
        // The only place the running torch feeds the watchdog.
        if feeder.tick() {
            hw::feed_watchdog();
        }
    }
}

// Turns the light off and spins. The watchdog isn't fed here, so it resets the
// chip within 64 WDT clocks of the last feed, about 63 ms, and the next boot
// comes up off, with the light capped.
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
