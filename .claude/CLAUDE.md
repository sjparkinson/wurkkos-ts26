# CLAUDE.md

Firmware for the Wurkkos TS26 torch (model 0721, ATtiny1616, MP3432 boost driver), in Rust, with no crate dependencies. README.md says what it is, CONTRIBUTING.md has the checks and rules, and docs/ has the hardware, the hazard log and the toolchain.

## Commands

```sh
cargo fmt --check
cargo test --workspace --locked -q     # plain `cargo test` skips xtask
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo clippy --package ts26 --lib --bins --examples --release --target avr-none -Zbuild-std=core -- -D warnings   # the only lint on hw.rs and rt.rs
cargo xtask hex                        # after any firmware change
```

The firmware clippy and xtask rebuild `core`, and the first run fetches crates, which the sandbox may block. Rerun outside it if so.

## Agents

The agents in `.claude/agents/` report back and never edit.

- `register-checker` after touching `src/regs.rs`, `src/avr/hw.rs` or `src/avr/rt.rs`.
- `avr-codegen` when `cargo xtask hex` fails to link, or flash or RAM grows.
- `hardware-researcher` before a new fact goes into docs/hardware.md.
- `microchip-docs` for what the datasheet, errata or an app note says. Its PDFs are in `docs/microchip/`, in the main checkout only.

## Working here

- Edit `.rs` files with Edit or Write, since the lint hook only runs on those.
- Parallel agents get their own worktree and files. Never `git stash`.
- The sandbox can't write `.git/config`, so push with `git push origin <branch>` and pass `--head <branch>` to `gh pr create`.
- PRs merge by rebase, so every commit is single-line and passes.
- Docs and comments are in documentation style, with no "I" or "we", and uncertainty kept as "likely" or "unconfirmed".
- Write a test or a `const` assert rather than a comment. A comment stays only as a `// SAFETY:`, a citation for a number, or "unconfirmed" on a guess, at a line each.
- docs/safety.md changes when a hazard or its cover changes, and not for every code change.

## Rules that bite

- **No dependencies**, in the firmware or xtask.
- **Only the non-HV UPDI programmer goes on this torch**, recovery included. The 12 V HV Friend never does, and it stays unplugged and off the bench while the torch is open, since both show as CH340 `1a86:7523`. An earlier torch was lost on 2026-11-06 after the Friend was held on its pads, and 12 V or a slipped probe is the suspected cause, unproven.
- **Never write fuses, the user row or a locked chip**, and never suggest a command that does. A wrong `SYSCFG0` loses the torch.
- **A torch that goes dark after a flash gets `cargo xtask peek`.**
- **Read programmer output for what it says.** "Nothing echoed back" can mean a short, `flash` only checks the signature, a verify doesn't prove the firmware runs, and `ping` and `peek` end with a reset. Ask which adapter is plugged in.
- **At the bench**, touch ground first, measure ohms between the pads before first contact, and leave the torch a few seconds with no cell after a flash. Never put the meter in series above 3 A, and never take the cell under 2.8 V.
- **Registers come from `src/regs.rs`**, generated from `vendor/ATtiny1616.atdf`. No bare numbers in `src/avr/`, and a number the ATDF lacks goes in `src/regs.rs` by hand with a comment.
- **PB2, the boost enable, is the dangerous pin.** It's low whenever the light's off, the duty goes before the enable on the way up and after it on the way down, and it never gets a pull-up. Neither does PA1 (the battery divider) or PA0 (UPDI).
- **Logic goes in the library**, which is `no_std`, has no `unsafe` and is tested on the host. `src/main.rs` stays a thin loop.
- **`testing::EXAMPLES` is the UI spec.** Change it in the same commit as the code.
- **The stock firmware is GPL-3.0.** Read it for hardware facts, copy none of its code or tables, and name it only in README.md's acknowledgements.
- **No runtime 32- or 64-bit multiply**, which includes dividing a `u16` by a constant that isn't a power of two. Host tests miss it, and `cargo xtask hex` fails with `__mulsi3`.
- **Flash is 16 KB and RAM is 2 KB.** No `core::fmt` and no floats.
- **The simulator finds mistakes and proves nothing.** It models no resets and nothing analogue, and it refuses a register it doesn't model, so teach it what the datasheet says. A pass never promotes anything in docs/hardware.md.

## Looks wrong but isn't

- `.rodata` is linked at 0x808000, the mapped-flash window in AVR ELF data space.
- `src/main.rs` has a host `fn main()` that exits with an error, so the workspace builds on the host.
