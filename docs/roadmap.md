# Roadmap

Roughly in order. Each step should leave a torch that's no worse than the step before. Tick things off here in the commit that does them. [breakout.md](breakout.md) gathers every check on a real light into one run.

## 1. It lights up

- [x] Toolchain with nightly `avr-none`, `rust-lld`, own linker script and vector table, no crates
- [x] Register constants generated from the ATtiny1616 ATDF at build time
- [x] Pin map for the model 0721 driver, from the stock firmware
- [x] Switch sampling at 64 Hz and a click/hold decoder
- [x] Click on, click off, at one dim level through the boost converter
- [x] The UPDI programmer in `xtask` (ping, backup, flash, restore), tested against a fake chip
- [ ] Find the flashing pads on a real TS26 and check their order with a multimeter
- [ ] Ping, back up and flash a real TS26, with only the plain UPDI adapter, and `first-light` (commit `9d3b7dc`) first
- [ ] Disassemble `backup/torch` and check what docs/hardware.md takes from the stock firmware's source against what the stock build really writes to the pins, TCA0 and the ADC
- [ ] Check the boost enable and PWM sequencing on a real light. Does it come on cleanly at duty 10, with no flash at turn-on?
- [ ] Measure what PWM duty does to output on the MP3432, so the ramp has something to stand on

## 2. It's safe to leave a cell in

A tick here means the firmware does it and it runs on the simulator. Nothing below has met a real light, so none of it means the torch is safe yet. [breakout.md](breakout.md) is what would.

- [x] Sleep when off, in power-down, with PB2 low first. Firmware only. The PIT wakes it every tick and the loop samples PA5. On the tick after a watchdog feed it idles instead, because the datasheet doesn't say a `wdr` finishes syncing with the peripheral clock stopped
- [ ] Test power-down straight after a feed on a real light (T4 in [breakout.md](breakout.md#test-builds)), then drop that idle if a resting torch doesn't reset. It costs about 0.37 mA, so this isn't fit for a drawer until then
- [ ] Battery voltage through the divider on PA1 (AIN1), not VCC. The firmware reads it with the stock firmware's slope of 1094. Still needs checking against a multimeter
- [x] Low-voltage protection. Firmware only, and every threshold's unmeasured. The Wurkkos 21700 is unprotected, so the firmware's the only thing between it and over-discharge. A cell it can't read holds the light at level 50 and turns it off after five minutes
- [x] Thermal regulation from the internal temperature sensor. Firmware only, and every constant's unmeasured. `ramp::CEILING` holds the light at level 120 until it's been run on a real light
- [x] A watchdog, so a hang can't leave the boost converter running. Firmware only, with a plain window and no proof at power-on, so nobody's seen it bite. The simulator can't show that either, since it doesn't model resets. The watchdog shares the tick's oscillator, so a tick that stops while the light's on has its own deadline, a count of PWM cycles, which turns the light off and powers down for good
- [x] Check flash against its CRC, and read the fuses back, at power-on. Firmware only. A failure, or a watchdog reset, caps the light at level 50. Neither check has seen a real chip

## Safety evidence

What covers each hazard, and how well, is in [safety.md](safety.md). These are the gaps it names that code can close. Like section 2, a tick means the firmware or the tests do it, and none of it is bench evidence.

- [x] No-panic link check. `cargo xtask hex` refuses a firmware image that links any panic path
- [x] Restriction lints on the library. Overflow, indexing, casts and unwraps are made explicit, and clippy denies the rest
- [x] Hold every ADC reading through low-voltage protection and thermal regulation and check where each ends up, plus a randomised property test of `limits::Limits`
- [x] Fault injection. Frozen plausible readings, a stuck switch, spurious ADC results, and, on the simulator, a dead ADC and a stuck switch
- [x] Unhandled interrupts go to `hw::halt` instead of `reti`
- [x] A commit stamp in flash, just before the CRC, shown by `flash` and `backup`

## 3. It's nice to use

- [x] A ramp, using PWM plus delta-sigma on TCA0 for a smooth low end, the way the stock firmware does it on this board. Firmware only, on this firmware's own curve
- [x] The RGB aux LEDs on PC1 to PC3. Firmware only
- [x] Remember the level in RAM, which a cell swap or any reset loses. Firmware only
- [x] A simulator. `cargo xtask hex` runs the firmware on a fake chip, and `cargo xtask simulate` runs the UI in a terminal. It finds mistakes and proves nothing
- ~~Battery check~~ Struck. No magic flashes, so no 3C voltage blink (docs/ui.md)

## Maybe

- Stop TCA0 while the light's off. It runs for good today, because a timer that stops and restarts is new turn-on behaviour nobody's checked, and what it'd save is small next to the idle after a watchdog feed. Worth a look once that idle has gone
