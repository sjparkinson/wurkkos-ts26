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
mod multiplies {
    use crate::hw;
    use ts26::ramp::Output;

    #[allow(unsafe_code)]
    #[unsafe(no_mangle)]
    extern "C" fn main() -> ! {
        hw::hold_enable_low();
        hw::init();
        loop {
            let dsm = hw::adc_result().unwrap_or(0) / 10;
            hw::set_output(Output { enable: false, dsm });
        }
    }

    #[panic_handler]
    fn panic(_: &core::panic::PanicInfo) -> ! {
        hw::halt()
    }
}

#[cfg(not(target_arch = "avr"))]
fn main() {
    eprintln!(
        "this is a check that a wide multiply fails to link for the ATtiny1616; xtask's tests \
         build it"
    );
    std::process::exit(1);
}
