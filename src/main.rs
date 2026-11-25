#![cfg_attr(target_arch = "avr", no_std, no_main)]
#![cfg_attr(target_arch = "avr", feature(asm_experimental_arch))]
#![cfg_attr(target_arch = "avr", feature(abi_avr_interrupt))]

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
