# Wurkkos TS26

Rust firmware for the Wurkkos TS26 torch (model 0721).

## Building

You need [rustup](https://rustup.rs).

```sh
cargo xtask hex
cargo test --workspace
```

See [docs/toolchain.md](docs/toolchain.md) for how it builds and links.

## Flashing

The TS26 has three flashing pads, `R - +`, for a plain serial UPDI programmer, never a high-voltage one. Take the cell out first. The first flash on any torch is T0, the click on/off image on branch `build-t0`, and the full firmware goes on once that works.

```sh
cargo xtask ping /dev/cu.usbserial-XXXX
cargo xtask backup /dev/cu.usbserial-XXXX backup/torch
cargo xtask flash /dev/cu.usbserial-XXXX [file.hex]
cargo xtask restore /dev/cu.usbserial-XXXX backup/torch
```

## Acknowledgements

- [Anduril](https://github.com/ToyKeeper/anduril), by ToyKeeper, the stock firmware, for the click notation and most of what's known about this hardware.
- [pymcuprog](https://github.com/microchip-pic-avr-tools/pymcuprog), which the UPDI programmer's sequences follow.
- [AVRDUDE](https://github.com/avrdudes/avrdude), whose flashing logic was reviewed when writing the UPDI programmer.
- Microchip, for the ATDF in `vendor/` and the datasheets.
