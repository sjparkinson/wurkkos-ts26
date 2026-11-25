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

The TS26 has three flashing pads, `R - +`, for a plain serial UPDI programmer, never a high-voltage one. Take the cell out first.

```sh
cargo xtask ping /dev/cu.usbserial-XXXX
cargo xtask backup /dev/cu.usbserial-XXXX backup/torch
cargo xtask flash /dev/cu.usbserial-XXXX [file.hex]
cargo xtask restore /dev/cu.usbserial-XXXX backup/torch
```

## Using it

- **Off.** 1C turns it on, at the same level each time, and 1H turns it on at the lowest level.
- **On.** 1C turns it off, and 1H ramps, each 1H going the other way from the last.
- **Lockout.** 2H locks it from on or off, confirmed with red aux for five seconds, and 2H again unlocks it to off.
- **Blue** aux shows while it's off, and goes dark when the cell's low.
- **Yellow** aux for five seconds after a press means the cell's flat.
- **White** aux means a fault, and it won't light again until the cell's taken out.

## Acknowledgements

- [Anduril](https://github.com/ToyKeeper/anduril), by ToyKeeper, the stock firmware, for the click notation and most of what's known about this hardware.
- [pymcuprog](https://github.com/microchip-pic-avr-tools/pymcuprog), which the UPDI programmer's sequences follow.
- [AVRDUDE](https://github.com/avrdudes/avrdude), whose flashing logic was reviewed when writing the UPDI programmer.
- Microchip, for the ATDF in `vendor/` and the datasheets.
