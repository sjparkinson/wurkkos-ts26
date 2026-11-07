# Contributing

## Checks

CI runs these on every pull request, so run them before you push.

```sh
cargo fmt --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo clippy --package ts26 --release --target avr-none -Zbuild-std=core -- -D warnings
cargo xtask hex
```

The first three run on your machine's own target. The second clippy lints the firmware as the AVR sees it, which is the only place `src/hw.rs` and `src/rt.rs` get linted. `cargo xtask hex` builds the real firmware and checks the linked image against what those two files assume (`xtask/src/layout.rs`: the vector table, where startup copies and zeroes memory, and that each protected-register unlock is followed straight away by its write). It does the same layout checks on `src/bin/link_probe.rs`, a throwaway program with a const table, a `.data` static and a `.bss` static, since the firmware has none of those yet. Then it runs both on the simulator. If that passes, it pads the image to 16 KB, puts the CRC in the last two bytes, and writes the file you'd flash. It prints how much of the 16 KB of flash and 2 KB of RAM the firmware uses, its stack, and how many instructions a tick and a PWM cycle take. CI keeps that hex and the ELF as a build artefact.

## The simulator

`xtask/src/sim.rs` puts a fake ATtiny1616 around a CPU in `xtask/src/avr.rs`, wired like the TS26, and runs the real firmware on it: from reset, through every example in docs/ui.md, through thousands of ticks of random clicks, and again with an interrupt landing on every instruction the loop reaches. It turns the rules a real chip enforces quietly into errors, such as PB2 high with no duty, a `wdr` in the closed window, or a handler that touches TCA0's shared TEMP byte. It refuses any register it doesn't model, so the first `cargo xtask hex` after you touch a new one fails until you teach it.

It's a mistake-finder, not proof. It's my reading of the datasheet, run against code written from that same reading, and it's never been compared with a real chip. A pass means the firmware does what the simulator thinks it should. What it leaves out:

- **Chip resets.** A watchdog bite is a failed run, so a real bite, and the boot after one, never runs anywhere. Neither does anything that happens to the pins during a reset.
- **Anything analogue.** The MP3432, whatever turns PB0's PWM into current, the cell, the divider's real ratio and VCC. The ADC reads scripted numbers.
- **Clocks and time.** It counts instructions, not cycles, and budgets by taking each as the slowest. The oscillators don't drift, and a `wdr` that's lost in power-down can't happen.
- **The flash itself.** Fuses are scripted, and the CRC runs over the image, so it shows `xtask` and the firmware agree with each other and not that the chip does.
- **The programmer on the pads.** Nothing here can see a slipped probe, or 12 V.

The UPDI programmer has its own, smaller fake chip in `xtask/src/updi.rs`, with the same caveat.

## Layout

```
src/lib.rs        the no_std library: everything testable, nothing hardware
src/button.rs     switch samples in, clicks and holds out (from the TS21)
src/ui.rs         the state machine docs/ui.md describes
src/torch.rs      one pass of the firmware loop: switch and ADC in, boost enable, duty and aux out
src/ramp.rs       the 150 levels, and the ceiling on them
src/limits.rs     what the cell and the temperature allow, from the UI's level to the level to run at
src/battery.rs    AIN1 to centivolts, and low-voltage protection
src/thermal.rs    thermal regulation
src/temp.rs       the temperature sensor's calibration
src/sensors.rs    which ADC conversion is due on which tick
src/gradual.rs    fading a ceiling a level at a time
src/watchdog.rs   which passes of the loop feed the watchdog, and the window that's sized for
src/selftest.rs   what the checks at power-on found
src/testing.rs    a fake ADC and readings, shared by the tests and the simulator
src/regs.rs       register addresses and bit values, checked against the ATDF
src/main.rs       the firmware loop (a stub on the host)
src/hw.rs         pins, register access and the interrupt handlers (AVR only)
src/rt.rs         vector table and startup (AVR only)
src/bin/link_probe.rs  a program with one static of each kind and a stub `mod hw`, so xtask can check link.x (never flashed)
link.x            memory layout for the linker, checked against the ATDF in xtask/src/atdf.rs
xtask/            hex, the simulator, simulate, and the UPDI programmer (ping, peek, backup, flash, restore)
docs/             hardware, flashing, bench plan, toolchain, UI spec, roadmap
vendor/           Microchip's ATDF for the part (Apache-2.0)
```

Keep logic in the library, where `cargo test` can reach it. `src/main.rs` should stay a loop that moves values between `hw` and the library.

## Registers

There's no PAC. When you need a register, add its address or bitfield value to `src/regs.rs`, taken from the ATtiny1616 ATDF in `vendor/ATtiny1616.atdf`, with a comment naming the register and bitfield. `src/hw.rs` uses the constant, not the number.

Then add a row for it to the tests in `xtask/src/atdf.rs`, which parse the ATDF and compare every constant in `src/regs.rs` against it. You can't forget: a constant no test reads is dead code in the xtask build, and clippy fails on it. The NVM and memory-map constants in `xtask/src/updi.rs` get the same check in that file's tests.

The tests only cover what the ATDF can say: addresses, widths, masks and named values. Write order, timing and which flags set without their interrupt enabled are in the datasheet, so have someone (or the `register-checker` agent in `.claude/agents/`) read those against it. A wrong address compiles fine and does something to the hardware you didn't ask for.

Never write fuses. Changing `SYSCFG0` can disable UPDI, and getting it back takes a high-voltage programmer, which never goes on this torch ([docs/flashing.md](docs/flashing.md#only-the-plain-adapter-goes-on-this-torch)). So that's the end of the light.

## Unsafe

`unsafe_code` is denied across the workspace. Only `src/hw.rs`, `src/rt.rs`, the `#[unsafe(no_mangle)]` on `main`, and the `no_mangle` items in `src/bin/link_probe.rs` opt out, and every `unsafe` block gets a `// SAFETY:` comment.

## Bumping the nightly

AVR is a tier 3 target. Pick a nightly, change `rust-toolchain.toml`, run every check above, and compare `cargo xtask hex` before and after. Then see whether the workarounds in [docs/toolchain.md](docs/toolchain.md) are still needed. The TS21 pins the same nightly, so bump them together.

## Commits

Single-line [Conventional Commits](https://www.conventionalcommits.org), such as `feat(ui): add a second level on 2C`. PRs merge by rebase only, so every commit on a branch lands on main.
