//! The firmware's loop. On the host it builds to a stub, so the workspace
//! builds and tests there.

#![cfg_attr(target_arch = "avr", no_std, no_main)]
#![cfg_attr(target_arch = "avr", feature(asm_experimental_arch))]
#![cfg_attr(target_arch = "avr", feature(abi_avr_interrupt))]
// Only the AVR clippy sees these, so it's the only check on hw.rs and rt.rs.
#![cfg_attr(
    target_arch = "avr",
    deny(
        clippy::indexing_slicing,
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::unreachable,
        clippy::arithmetic_side_effects,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_possible_wrap,
        clippy::as_conversions,
    )
)]

#[cfg(target_arch = "avr")]
mod avr {
    pub mod hw;
    mod rt;
}
#[cfg(target_arch = "avr")]
use avr::hw;
#[cfg(target_arch = "avr")]
use ts26::regs;

#[cfg(target_arch = "avr")]
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
extern "C" fn main() -> ! {
    use ts26::selftest::{self, Faults};
    use ts26::torch::{Changes, Torch};
    use ts26::watchdog::{Pace, Step};

    // First, since PB2 floats from reset until it's an output.
    hw::hold_enable_low();

    let faults = Faults {
        hang: selftest::hung(hw::take_reset_flags()),
        flash: !hw::flash_ok(),
        fuses: !hw::fuses().ok(),
    };

    hw::init();
    hw::start_watchdog();

    let mut torch = Torch::new(hw::temperature_calibration(), faults);
    let mut output_changes = Changes::new();
    let mut aux_changes = Changes::new();
    let mut pace = Pace::new();

    loop {
        let woken = if pace.slow() {
            !hw::rest()
        } else if hw::wait_for_tick(torch.resting() && pace.powers_down()) {
            false
        } else {
            // Most likely a stopped 32 kHz oscillator with the light on, which
            // stops the watchdog too.
            hw::halt();
        };

        let step = if woken {
            pace.woken()
        } else {
            torch.tick(hw::switch_pressed(), hw::adc_result(), pace.ticks());

            if let Some(convert) = torch.convert() {
                hw::adc_start(convert);
            }

            if let Some(output) = output_changes.update(torch.output()) {
                hw::set_output(output);
            }

            if let Some(aux) = aux_changes.update(torch.aux()) {
                hw::set_aux(aux);
            }

            pace.tick(torch.slow())
        };

        match step {
            Some(Step::Feed) => hw::feed_watchdog(),
            Some(Step::Slow) => hw::slow_down(),
            Some(Step::Fast) => hw::speed_up(),
            Some(Step::Window) => hw::window_watchdog(),
            None => {}
        }
    }
}

// `halt` rather than a spin, which would drain the cell if the 32 kHz
// oscillator, and so the watchdog, had stopped.
#[cfg(target_arch = "avr")]
#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    hw::halt()
}

#[cfg(not(target_arch = "avr"))]
fn main() {
    eprintln!("this is firmware for the ATtiny1616; build it with `cargo xtask hex`");
    std::process::exit(1);
}
