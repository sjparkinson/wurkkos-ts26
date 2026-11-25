#![expect(
    clippy::doc_markdown,
    reason = "the generated docs are the ATDF's names and captions as Microchip wrote them"
)]

include!(concat!(env!("OUT_DIR"), "/regs.rs"));

/// Not in the ATDF: ADC0.SAMPCTRL.SAMPLEN is a count, here 20 more ADC clocks of sampling.
pub const ADC_SAMPLEN_32US: u8 = 20;
const _: () = assert!(ADC_SAMPLEN_32US & !ADC_SAMPLEN == 0);
