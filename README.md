# wurkkos-ts26

Rust firmware for the Wurkkos TS26 torch (Anduril model 0721), with a much smaller UI than [Anduril](https://github.com/ToyKeeper/anduril). It's a sibling of [wurkkos-ts21](https://github.com/sjparkinson/wurkkos-ts21) and borrows its toolchain, its approach and a fair bit of its code.

> [!WARNING]
> This isn't firmware to carry yet. It has never run on a real TS26. Its sleep, low-voltage protection, thermal regulation and watchdog have only run on a simulator, which finds mistakes and proves nothing, and every threshold behind them is unmeasured on this light. The cell's unprotected and turbo pulls about 11 A, so level 120 of 150 is the most it'll do until that changes. See the [roadmap](docs/roadmap.md) and the [bench plan](docs/bench-plan.md).

## Using it

From off, 1C turns it on at the last level, 1H gives the lowest level, 2C gives turbo, and 4C locks it. While it's on, 1H ramps up, 2H ramps down, 2C goes to turbo and back, and 1C turns it off. The aux LEDs glow dim blue when off and dim red in lockout. It never flashes to tell you anything. [docs/ui.md](docs/ui.md) is the spec.

## Building

You need [rustup](https://rustup.rs). The pinned nightly, `rust-src` and everything else come down on the first build.

```sh
cargo xtask hex     # build the firmware, run it on the simulator, write target/wurkkos-ts26-0721.hex and print flash and RAM use
cargo test --workspace
cargo xtask simulate 1C 2H:1.5s wait:1s 3C   # try the UI on a script, or leave the script off to try it live
```

See [docs/toolchain.md](docs/toolchain.md) for how it builds and links.

## Flashing

The TS26 has flashing pads for a UPDI programmer. Put a plain serial UPDI programmer on them and `cargo xtask` does the rest. Never use a high-voltage one: I lost a TS21 after hand-holding one on its pads, and [docs/flashing.md](docs/flashing.md#only-the-plain-adapter-goes-on-this-torch) has what happened.

Check it can see the chip, back up the factory firmware, then flash `first-light` (commit `9d3b7dc`), a 628-byte image that clicks on and off and does nothing else. The full firmware only goes on once that's working on the light.

```sh
cargo xtask ping /dev/cu.usbserial-XXXX
cargo xtask backup /dev/cu.usbserial-XXXX
git switch --detach 9d3b7dc
cargo xtask flash /dev/cu.usbserial-XXXX
git switch -
```

See [docs/flashing.md](docs/flashing.md) for programmers, the pads, restoring the backup and getting back to Anduril, [docs/bench-plan.md](docs/bench-plan.md) for every check to make on a real light and in what order, and [docs/hardware.md](docs/hardware.md) for what's inside and how sure anyone is about it.
