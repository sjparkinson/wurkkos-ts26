//! The vector table and what runs between reset and `main`.

#![allow(unsafe_code)]

use core::arch::global_asm;

use crate::regs;

// The table below lists the handled vectors in this order.
const _: () = assert!(
    0 < regs::VECTOR_PORTA_PORT
        && regs::VECTOR_PORTA_PORT < regs::VECTOR_RTC_PIT
        && regs::VECTOR_RTC_PIT < regs::VECTOR_TCA0_OVF
        && regs::VECTOR_TCA0_OVF < regs::VECTORS
);

global_asm!(
    // 31 slots, each a two-word `jmp`.
    ".section .vectors, \"ax\", @progbits",
    ".global __vectors",
    "__vectors:",
    "  jmp __reset",
    "  .rept {before_porta}",
    "  jmp __unhandled_interrupt",
    "  .endr",
    "  jmp {porta_port}",
    "  .rept {before_pit}",
    "  jmp __unhandled_interrupt",
    "  .endr",
    "  jmp {rtc_pit}",
    "  .rept {before_ovf}",
    "  jmp __unhandled_interrupt",
    "  .endr",
    "  jmp {tca0_ovf}",
    "  .rept {after_ovf}",
    "  jmp __unhandled_interrupt",
    "  .endr",
    "",
    // No `reti`, which would return with the interrupt's flag still up and
    // take it again after every instruction (DS40002204A section 13.2), with
    // PB2 possibly high. r1 is zeroed since an interrupt can land straight
    // after a `mul`.
    ".section .text.__unhandled_interrupt, \"ax\", @progbits",
    "__unhandled_interrupt:",
    "  eor r1, r1",
    "  jmp {halt}",
    "",
    ".section .text.__reset, \"ax\", @progbits",
    ".global __reset",
    "__reset:",
    // Compiled code assumes r1 is zero. Reset leaves SP at the top of SRAM.
    "  clr r1",
    "  out {sreg}, r1",
    "  ldi r26, lo8(__data_start)",
    "  ldi r27, hi8(__data_start)",
    "  ldi r30, lo8(__data_load)",
    "  ldi r31, hi8(__data_load)",
    "  ldi r24, lo8(__data_end)",
    "  ldi r25, hi8(__data_end)",
    // The test comes first, so an empty .data copies nothing.
    "  rjmp 2f",
    "1:",
    "  ld r0, Z+",
    "  st X+, r0",
    "2:",
    "  cp r26, r24",
    "  cpc r27, r25",
    "  brne 1b",
    "  ldi r26, lo8(__bss_start)",
    "  ldi r27, hi8(__bss_start)",
    "  ldi r24, lo8(__bss_end)",
    "  ldi r25, hi8(__bss_end)",
    "  rjmp 4f",
    "3:",
    "  st X+, r1",
    "4:",
    "  cp r26, r24",
    "  cpc r27, r25",
    "  brne 3b",
    "  call main",
    // `main` never returns. This keeps a return out of what's next in flash.
    "5:",
    "  rjmp 5b",
    before_porta = const regs::VECTOR_PORTA_PORT - 1,
    porta_port = sym super::hw::porta_port,
    before_pit = const regs::VECTOR_RTC_PIT - regs::VECTOR_PORTA_PORT - 1,
    rtc_pit = sym super::hw::rtc_pit,
    before_ovf = const regs::VECTOR_TCA0_OVF - regs::VECTOR_RTC_PIT - 1,
    tca0_ovf = sym super::hw::tca0_ovf,
    after_ovf = const regs::VECTORS - regs::VECTOR_TCA0_OVF - 1,
    halt = sym super::hw::halt,
    sreg = const regs::addr::CPU_SREG,
);
