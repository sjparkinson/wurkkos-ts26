# Contributing

## Checks

CI runs these on every pull request, so run them before you push.

```sh
cargo fmt --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked -- --include-ignored
cargo clippy --package ts26 --lib --bins --examples --release --target avr-none -Zbuild-std=core -- -D warnings
cargo xtask hex
```

The second clippy is the only place `src/avr/hw.rs` and `src/avr/rt.rs` get linted. `cargo xtask hex` builds the firmware, checks the linked image (`xtask/src/layout.rs`), runs it on a simulated chip (`xtask/src/sim.rs`) through every row of `testing::EXAMPLES` and thousands of random ticks, then writes the hex with a commit stamp and CRC and prints flash and RAM use. The files in `examples/` are never flashed. `link.rs` and `guards.rs` exist so xtask can check `link.x` and run `hw.rs`'s guards. `panics.rs` indexes out of bounds and `multiplies.rs` divides a `u16` by ten, and xtask's tests build both for the chip, failing unless the panic check refuses the first and the linker refuses the second.

A rule in `xtask/src/sim.rs` or `xtask/src/layout.rs` gets a test beside it that drives the model with the mistake it refuses. A rule that only bites in a state the firmware has to be driven into also gets a check that the run got there, as `Report::exercised` does. The simulator's runs go in parallel, and when more than one fails, `hex` reports the one that failed after the fewest instructions and stops the rest.

One test is ignored, since it takes about ten seconds, and `--include-ignored` runs it. It breaks copies of the firmware in one way at a time (`xtask/src/mistakes.rs`) and wants the checks `hex` runs to refuse each for its own reason. It takes the tree as it stands to pass `cargo xtask hex`, and only checks that an unbroken copy does when a mistake isn't refused as expected. Run it after touching `xtask/src/sim.rs`, `xtask/src/layout.rs`, `src/avr/hw.rs`, `src/avr/rt.rs` or `src/main.rs`, and only add a mistake once you've seen it refused for the reason you meant.

The simulator finds mistakes and proves nothing. It's one reading of the datasheet, run against code written from the same reading. It doesn't model chip resets (a watchdog bite is a failed run), anything analogue (the driver, the cell, the divider), real time or a probe on the pads, so a firmware change still wants reading against the datasheet before it's flashed. It refuses a register it doesn't model, so teach it what the datasheet says.

## Rules

- **No dependencies**, in the firmware or in xtask.
- **Logic goes in the library** (`src/lib.rs` and its modules), which is `no_std`, has no `unsafe` and is tested on the host. `src/main.rs` stays a thin loop.
- **`testing::EXAMPLES` is the UI spec** (`src/testing/examples.rs`). Change the row with the code, and add one for new behaviour.
- **Registers come from `src/regs.rs`**, which `build.rs` generates from `vendor/ATtiny1616.atdf`. A number the ATDF doesn't have goes in by hand, with a comment saying why. Picking the wrong constant still compiles, so have the datasheet read against register code.
- **`unsafe` lives in `src/avr/`**, plus `main`'s `no_mangle` and the examples, and every block gets a `// SAFETY:` comment.
- **Never write fuses**, and only a plain UPDI adapter goes on the torch, never a high-voltage one. A wrong `SYSCFG0` turns UPDI off, and getting it back takes 12 V, which this torch never gets. README.md has the flashing commands.

## Comments

The code is the source of truth, so a comment has to say something the code can't. Keep these, at a line each.

- `// SAFETY:` on an `unsafe` block.
- Where a number comes from, such as `// DS40002204A §30.3.1.1`.
- "Unconfirmed" or "unmeasured" on a guess.

A trap, where the obvious edit would be wrong, gets a test or a `const` assert with the reason as its message, not a comment. Leave out anything that restates a name or a signature, explains how the code works, narrates a test, records history, or points at a document. A test's name is its description.

## Commits

Single-line [Conventional Commits](https://www.conventionalcommits.org), such as `feat(ui): add a second level on 2C`. PRs merge by rebase only, so every commit on a branch lands on main and has to pass.

## Bumping the nightly

AVR is a tier 3 target and nightlies do break it. Change `rust-toolchain.toml`, run every check above, and compare `cargo xtask hex` before and after. `cargo test` fails if the panic check no longer knows the new nightly's panic symbols, or if a wide multiply has started to link.
