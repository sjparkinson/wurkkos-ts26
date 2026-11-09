# Wurkkos TS26

Rust firmware for the Wurkkos TS26 torch (model 0721).

> [!WARNING]
> This isn't firmware to carry yet. It has never run on a real TS26. Its sleep, low-voltage protection, thermal regulation and watchdog have only run on a simulator, which finds mistakes and proves nothing, and every threshold behind them is unmeasured on this light. See the [roadmap](docs/roadmap.md) and the [bench plan](docs/bench-plan.md).

## Using it

From off, 1C turns it on and another 1C turns it off. See [docs/ui.md](docs/ui.md) for the full UI.

## Building

You need [rustup](https://rustup.rs).

```sh
cargo xtask hex
cargo test --workspace
```

See [docs/toolchain.md](docs/toolchain.md) for how it builds and links.

## Flashing

The TS26 has flashing pads for a UPDI programmer. Put a plain serial UPDI programmer on them and `cargo xtask` does the rest.

```sh
cargo xtask ping /dev/cu.usbserial-XXXX
cargo xtask backup /dev/cu.usbserial-XXXX backup/torch
cargo xtask flash /dev/cu.usbserial-XXXX
```

See [docs/flashing.md](docs/flashing.md) for programmers, the pads, and restoring the backup.
