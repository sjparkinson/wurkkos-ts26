//! ATtiny1616 register addresses and bitfield values, generated at build time
//! from the ATDF. The ATDF's reset values aren't here, since it disagrees with
//! the datasheet on some of them.

include!(concat!(env!("OUT_DIR"), "/regs.rs"));

/// ADC0.SAMPCTRL.SAMPLEN as 20 more ADC clocks of sampling, a count chosen
/// here rather than a value the ATDF names.
pub const ADC_SAMPLEN_32US: u8 = 20;
const _: () = assert!(ADC_SAMPLEN_32US & !ADC_SAMPLEN == 0);
