# Roadmap

Roughly in order. Each step should leave a torch that's no worse than the step before. Tick things off here in the commit that does them. Where the TS21 has already done the work, [wurkkos-ts21](https://github.com/sjparkinson/wurkkos-ts21) is the place to borrow it from, after checking what's different about this board. [bench-plan.md](bench-plan.md) gathers every check on a real light into one run.

## 1. It lights up

- [x] Toolchain with nightly `avr-none`, `rust-lld`, own linker script and vector table, no crates (from the TS21)
- [x] Register constants checked against the ATtiny1616 ATDF
- [x] Pin map for the model 0721 driver, from Anduril's `hw/wurkkos/ts26`
- [x] Switch sampling at 64 Hz and a click/hold decoder (from the TS21)
- [x] Click on, click off, at one dim level through the boost converter
- [x] The UPDI programmer in `xtask` (ping, backup, flash, restore), tested against a fake chip (from the TS21)
- [ ] Find the flashing pads on a real TS26 and check their order with a multimeter
- [ ] Ping, back up and flash a real TS26, with only the plain UPDI adapter, and `first-light` (commit `9d3b7dc`) first
- [ ] Check the boost enable and PWM sequencing on a real light: does it come on cleanly at duty 10, with no flash at turn-on?
- [ ] Measure what PWM duty does to output on the MP3432, so the ramp has something to stand on

## 2. It's safe to leave a cell in

A tick here means the firmware does it and it runs on the simulator. Nothing below has met a real light, so none of it means the torch is safe yet. [bench-plan.md](bench-plan.md) is what would.

- [x] Sleep when off, in power-down, with PB2 low first. Firmware only. The PIT wakes it every tick and the loop samples PA5. On the tick after a watchdog feed it idles instead, because the datasheet doesn't say a `wdr` finishes syncing with the peripheral clock stopped
- [ ] Test power-down straight after a feed on a real light (T4 in [bench-plan.md](bench-plan.md#test-builds)), then drop that idle if a resting torch doesn't reset. It costs about 0.37 mA, so this isn't fit for a drawer until then
- [ ] Battery voltage through the divider on PA1 (AIN1), not VCC. The firmware reads it with Anduril's slope of 1094. Still needs checking against a multimeter
- [x] Low-voltage protection. Firmware only, and every threshold's unmeasured. The Wurkkos 21700 is unprotected, so the firmware's the only thing between it and over-discharge
- [x] Thermal regulation from the internal temperature sensor. Firmware only, and every constant's unmeasured. `ramp::CEILING` holds the light at level 120 until it's been run on a real light
- [x] A watchdog, so a hang can't leave the boost converter running. Firmware only: a plain window, with no proof at power-on, so nobody's seen it bite. The simulator can't show that either, since it doesn't model resets
- [x] Check flash against its CRC, and read the fuses back, at power-on. Firmware only. A failure, or a watchdog reset, caps the light at level 50. Neither check has seen a real chip

## 3. It's nice to use

- [x] A ramp, using PWM plus delta-sigma on TCA0 for a smooth low end, the way Anduril does it on this board. Firmware only, on my own curve
- [x] The RGB aux LEDs on PC1 to PC3. Firmware only
- [x] Remember the level in RAM, which a cell swap or any reset loses. Firmware only
- [x] A simulator: `cargo xtask hex` runs the firmware on a fake chip, and `cargo xtask simulate` runs the UI in a terminal. It finds mistakes and proves nothing
- ~~Battery check~~ Struck: no magic flashes, so no 3C voltage blink (docs/ui.md)

## Maybe

- Share a crate with the TS21 for the bits that are the same (button decoder, UPDI programmer, toolchain glue), once both have settled. Zero dependencies still applies, so that would be a path dependency in a shared repo, not crates.io
