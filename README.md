# wurkkos-ts26

Rust firmware for the Wurkkos TS26 torch (Anduril model 0721), with a much smaller UI than [Anduril](https://github.com/ToyKeeper/anduril). It's a sibling of [wurkkos-ts21](https://github.com/sjparkinson/wurkkos-ts21) and borrows its toolchain, its approach and a fair bit of its code.

> [!WARNING]
> This is a framework, not firmware you'd carry. It turns the light on and off at one dim level and does nothing else: no sleep when off (it'll flatten a cell in days), no low-voltage protection and no thermal regulation. The TS26 has a boost driver, so the risks differ from the TS21's FET, but they're no smaller. See the [roadmap](docs/roadmap.md).

## Using it

1C turns it on, 1C turns it off. That's the whole UI for now. [docs/ui.md](docs/ui.md) is the spec.

## Building

You need [rustup](https://rustup.rs). The pinned nightly, `rust-src` and everything else come down on the first build.

```sh
cargo xtask hex     # build the firmware, write target/wurkkos-ts26-0721.hex and print flash and RAM use
cargo test --workspace
```

See [docs/toolchain.md](docs/toolchain.md) for how it builds and links.

## Flashing

The TS26 has flashing pads for a UPDI programmer. Put a plain serial UPDI programmer on them and `cargo xtask` does the rest. Never use a high-voltage one: I lost a TS21 after hand-holding one on its pads, and [docs/flashing.md](docs/flashing.md#only-the-plain-adapter-goes-on-this-torch) has what happened.

Check it can see the chip, back up the factory firmware, then flash the `first-light` tag, a 628-byte image that clicks on and off and does nothing else. The full firmware only goes on once that's working on the light.

```sh
cargo xtask ping /dev/cu.usbserial-XXXX
cargo xtask backup /dev/cu.usbserial-XXXX
git switch --detach first-light
cargo xtask flash /dev/cu.usbserial-XXXX
git switch -
```

See [docs/flashing.md](docs/flashing.md) for programmers, the pads, restoring the backup and getting back to Anduril, [docs/bench-plan.md](docs/bench-plan.md) for every check to make on a real light and in what order, and [docs/hardware.md](docs/hardware.md) for what's inside and how sure anyone is about it.
