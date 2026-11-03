# Roadmap

Roughly in order. Each step should leave a torch that's no worse than the step before. Tick things off here in the commit that does them. Where the TS21 has already done the work, [wurkkos-ts21](https://github.com/sjparkinson/wurkkos-ts21) is the place to borrow it from, after checking what's different about this board.

## 1. It lights up

- [x] Toolchain with nightly `avr-none`, `rust-lld`, own linker script and vector table, no crates (from the TS21)
- [x] Register constants checked against the ATtiny1616 ATDF
- [x] Pin map for the model 0721 driver, from Anduril's `hw/wurkkos/ts26`
- [x] Switch sampling at 64 Hz and a click/hold decoder (from the TS21)
- [x] Click on, click off, at one dim level through the boost converter
- [x] The UPDI programmer in `xtask` (ping, backup, flash, restore), tested against a fake chip (from the TS21)
- [ ] Find the flashing pads on a real TS26 and check their order with a multimeter
- [ ] Ping, back up and flash a real TS26
- [ ] Check the boost enable and PWM sequencing on a real light: does it come on cleanly at duty 10, with no flash at turn-on?
- [ ] Measure what PWM duty does to output on the MP3432, so the ramp has something to stand on

## 2. It's safe to leave a cell in

- [ ] Sleep when off, in power-down, woken by the switch on PA5. Borrow the TS21's plan, and make sure PB2 is low before sleeping so the boost converter's off
- [ ] Battery voltage through the divider on PA1 (AIN1), not VCC. Work out the divider and reference, and check against a multimeter
- [ ] Low-voltage protection. The Wurkkos 21700 is unprotected, so the firmware's the only thing between it and over-discharge
- [ ] Thermal regulation from the internal temperature sensor
- [ ] A watchdog, so a hang can't leave the boost converter running

## 3. It's nice to use

- [ ] A ramp, using PWM plus delta-sigma on TCA0 for a smooth low end, the way Anduril does it on this board
- [ ] The RGB aux LEDs on PC1 to PC3
- [ ] Battery check
- [ ] Remember the level in EEPROM

## Maybe

- A UI simulator, like the TS21's `cargo xtask simulate`
- Share a crate with the TS21 for the bits that are the same (button decoder, UPDI programmer, toolchain glue), once both have settled. Zero dependencies still applies, so that would be a path dependency in a shared repo, not crates.io
