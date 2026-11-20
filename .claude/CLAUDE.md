# CLAUDE.md

Firmware for the Wurkkos TS26 torch (model 0721) (ATtiny1616, MP3432 boost driver), in Rust, with zero crate dependencies, and deliberately small. Read README.md for what it is, CONTRIBUTING.md for the checks and rules, and docs/ for the hardware, the hazard log and the toolchain. The full firmware runs on the torch, and docs/hardware.md lists what's been seen there and what hasn't been measured.

## Commands

```sh
cargo fmt --check                                          # formatting, as CI runs it
cargo test --workspace --locked -q                         # host tests, run these constantly
cargo clippy --workspace --all-targets --locked -- -D warnings   # host lints
cargo clippy --package ts26 --lib --bins --examples --release --target avr-none -Zbuild-std=core -- -D warnings   # firmware lints: the only place hw.rs and rt.rs get linted
cargo xtask hex                                            # firmware ELF and hex, plus flash/RAM use, after any firmware change
cargo xtask breakages                                      # about a minute: hex refuses each deliberately broken copy, after touching sim.rs, layout.rs or a breakage's lines
```

CONTRIBUTING.md lists every check CI runs. Plain `cargo test` only tests the root package, so always pass `--workspace`.

The firmware clippy and the xtask commands rebuild `core` with `-Zbuild-std`. The first run on a machine fetches crates from crates.io and writes to `~/.cargo` and `~/.rustup`, which the sandbox may block. If it does, rerun outside the sandbox; nothing else in this repo needs that.

## Agents

Agents in `.claude/agents/` report back and never edit. Use them rather than doing the same work inline.

- `register-checker` is for after touching `src/regs.rs`, `src/avr/hw.rs` or `src/avr/rt.rs`.
- `avr-codegen` is for when `cargo xtask hex` fails to link, or flash or RAM grows and you want to know why.
- `hardware-researcher` is for before writing a new fact into docs/hardware.md.
- `microchip-docs` is for when you need what the datasheet, errata or an app note says. It searches the local PDFs in `docs/microchip/`, which are gitignored and only exist in the main checkout.

## Working in this repo

- **Edit `.rs` files with Edit or Write**, not `sed -i`, `cp` or a heredoc. The lint hook only runs on Edit and Write.
- **Parallel agents get their own worktree and their own files.** Never `git stash` to compare before and after; use a WIP commit or a second worktree.
- **The sandbox can't write `.git/config`**, so `git push -u` pushes but can't record the upstream. Push with `git push origin <branch>`, pass `--head <branch>` to `gh pr create`, and don't try to fix the tracking.
- **PRs merge by rebase only**, so every commit on a branch lands on main as written. Keep each one single-line and passing.
- **Docs and comments are in documentation style.** No "I", "I've", "my", "we" or "our". Describe the thing ("the firmware reads", "nobody has measured"), and use "you" only for instructions to the reader. Keep uncertainty as words like "likely" or "unconfirmed" rather than "I think". Quotes stay as they were written.
- **The code is the source of truth, so write few comments and no new docs.** A comment stays only if it's a `// SAFETY:`, a citation for a number or an ordering, a trap no test catches, or an "unconfirmed" on a guess, at a line or two each (CONTRIBUTING.md has the rule). Never add one that restates code, explains how it works, narrates a test, records history or points at a document, and leave alone the comments a change doesn't affect.
- **docs/safety.md is the hazard log.** Change it when a hazard is added or removed, or when what covers one changes in kind, and not for every code change.

## Rules that bite

- **No dependencies.** Not in the firmware, not in xtask. If something seems to need a crate, write the small part you need, or ask first.
- **Only the non-HV UPDI programmer ever goes on this torch.** The 12 V HV Friend never does, recovery included, so never suggest it or any other high-voltage programmer for the TS26. An earlier torch was lost on 2026-11-06 after the Friend was hand-held on its pads, and it then measured about 2.5 Ω between all three. The user thinks 12 V or a slipped probe is the likely cause, and that isn't proven, so don't write it up as if it were. The Friend puts 12 V on the UPDI line for about 100 µs on every falling edge of RTS, which likely means every time a tool opens the port, and no switch or jumper turns that off. Both it and the plain kit show as CH340 `1a86:7523` with the same kind of port name, so nothing on the computer tells them apart. The Friend stays unplugged and off the bench whenever the torch is open.
- **The first flash on any torch is T0** (first light, commit `9d3b7dc`, branch `build-t0`), the 628-byte click on/off image. The full firmware only goes on once that's proven on the light. A torch that goes dark after a flash gets T0 again, after `cargo xtask peek` (only its first run shows how far the firmware got).
- **Read programmer output for what it really says.** A pulled-down UPDI line reads "nothing echoed back", not "no reply", so that message on the pads can mean a short rather than a bad adapter. Both adapters show as CH340 `1a86:7523`, so ask which one's plugged in. `flash` only checks the signature, and a verify doesn't prove the firmware runs. `ping` and `peek` finish with UPDIDIS, which DS §33.3.2.2.2 says issues a System Reset, so don't call them harmless to a running chip. An earlier torch's pads measured about 1.75 mm pitch, not 1.27, so measure before trusting a probe.
- **At the bench**, touch ground before the probe lands, measure ohms between the pads before first contact, and leave the torch a few seconds with no cell after a flash before powering it up. Never put the meter in series at the tail above 3 A, and don't take the cell under 2.8 V.
- **Never write fuses, and never suggest a command that does.** A wrong `SYSCFG0` disables UPDI, and only a high-voltage programmer gets it back, which this torch never gets, so it would be lost. `cargo xtask flash` writes flash only.
- **The UPDI programmer never writes fuses, the user row or a locked chip.** Don't add any of that without the user asking for it explicitly.
- **Every register address and bit mask comes from `src/regs.rs`, which `build.rs` generates from the ATDF** (`vendor/ATtiny1616.atdf`) with `generate` in `xtask/src/atdf.rs`. No bare register numbers in `src/avr/hw.rs` or `src/avr/rt.rs`, and nothing typed by hand that the ATDF names, so use the generated constant. A number the ATDF doesn't have, like `ADC_SAMPLEN_32US`, goes in `src/regs.rs` by hand with a comment saying why. Use the `register-checker` agent after touching `src/regs.rs`, `src/avr/hw.rs` or `src/avr/rt.rs`, for what the ATDF can't express.
- **16-bit peripheral registers go low byte first.** Use `write16` in `src/avr/hw.rs`.
- **The boost converter's enable (PB2) is the dangerous pin.** It's low whenever the light's off, including at reset and in `hw::halt`, which is also the panic handler. Turn on by setting the duty and then raising the enable; turn off by dropping the enable and then zeroing the duty. Never give PB2 a pull-up.
- **PA1 is analogue (the battery divider) and PA0 is UPDI.** `hw::init` turns PA1's digital input buffer off and writes nothing else to either pin. Don't add a pull-up to either.
- **Logic goes in the library** (`src/lib.rs` and its modules), which is `no_std`, has no `unsafe`, and is tested on the host. `src/main.rs` stays a thin loop.
- **`testing::EXAMPLES` is the UI spec** (`src/testing/examples.rs`), which `cargo test` plays on the library and `cargo xtask hex` on the firmware. Change it in the same commit as the code, and add a row for new behaviour.
- **The stock firmware is GPL-3.0.** Read its source for facts about the hardware (pins, calibration, timings), and don't copy its code or its ramp tables into this MIT repo. Docs and comments call it "the stock firmware" and name none of its files, identifiers or commits. README.md's acknowledgements are the only place that names or links it.
- **No runtime 32- or 64-bit multiplication.** The chip has an 8×8 hardware multiplier, so 8- and 16-bit multiplies compile to `mul` instructions, but there's no libgcc, so anything wider fails to link with `undefined symbol: __mulsi3`. That includes dividing a `u16` by a constant that isn't a power of two (`x / 10`, `x % 10`), which LLVM turns into a 32-bit multiply. Host tests won't catch it; `cargo xtask hex` will. 8-bit maths, 16-bit multiplies, division by a runtime value, and `const fn` maths are fine. docs/toolchain.md has the options.
- **Flash is 16 KB and RAM is 2 KB.** Check `cargo xtask hex` after changes. No `core::fmt` in the firmware (it pulls in kilobytes), and no floats.
- **Mark uncertainty.** docs/hardware.md labels facts as confirmed or likely. Keep doing that; don't promote a guess to a fact, and don't write anything up as seen on this torch until someone has seen it.
- **The simulator finds mistakes and proves nothing.** `cargo xtask hex` runs the firmware on a fake chip (`xtask/src/sim.rs`), but that chip is one reading of the datasheet, run against code written from the same reading, and it's never been compared with a real one. It doesn't model chip resets, so a watchdog bite is a failed run and what follows one never runs. It doesn't model anything analogue either: the driver, the cell, or a probe on the pads. A firmware change still needs reading against the datasheet (the `register-checker` agent) before anyone flashes it, and a pass never promotes anything in docs/hardware.md. A register it doesn't model is an error, so teach it what the datasheet says rather than what makes the build pass.

## Things that look wrong but aren't

- `.rodata` is linked at 0x808000 and up, because AVR ELF files put data space at 0x800000, plus the mapped-flash window at 0x8000. It's how const tables are read without `lpm`.
- `src/main.rs` has a host `fn main()` that exits with an error. It's there so the workspace builds and tests on the host.
- `src/main.rs` powers down on the tick after a watchdog feed too, except the one after the window comes back, though the datasheet doesn't say a `wdr` finishes syncing with the peripheral clock stopped. T4 (branch `build-t4-main`) did the same on the torch from 2026-11-16 for several days with no reset or bite.
- At rest the PIT ticks at 1 Hz and the watchdog runs 8 s with no window (`watchdog::Pace`), and a press on PA5 wakes it. The window only comes back on the second 64 Hz tick, after a `wdr` on the first, since nothing says whether a `WDT.CTRLA` write restarts the count or where the PIT's first tick after a change lands.
- `hw::wait_for_tick` only has a deadline while PB0's PWM runs (lit, or charging the driver's filter with PB2 low during a press), about 104 ms with no tick, then `main` calls `hw::halt`, which turns the light off and powers down for good. With the PWM stopped there's none, since a dead 32 kHz oscillator then leaves the chip asleep with PB2 low, which is the safe state.
- A press from off runs PB0's PWM with PB2 low, at the level the press would light, so the filter between PB0 and the driver is charged before the enable rises.
