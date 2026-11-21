# Hardware

What's known about a Wurkkos TS26 (model 0721). **Confirmed** means a primary source says so (Microchip, a datasheet, the stock firmware's source). **Likely** means it follows from a sibling light or a reviewer. **On this torch** means it was read or seen on this project's TS26. No teardown of a TS26 has been found.

A version check from off on the stock firmware (15 or more clicks) blinks the model number, 0721. The TS26S shares the body and isn't a 0721.

## Pins

| Pin | Job | Notes |
|---|---|---|
| PA0 | UPDI | The programming pad |
| PA1 | Battery sense | Through a divider, ADC AIN1, digital input buffer off |
| PA5 | Side switch | Active low, internal pull-up |
| PB0 | Brightness | TCA0 WO0, PWM into the current control |
| PB2 | Boost enable | High runs the boost converter. Never gets a pull-up |
| PC1, PC2, PC3 | Aux blue, green, red | Plain GPIO |
| VCC | MCU supply | Regulated, not the cell |

The other pins are unused and get the pull-up. Confirmed from the stock firmware. Blue and red are seen on this torch, green isn't.

The fuses on this torch are the factory defaults, `00 00 02 FF 00 F6 07 00 00 FF C5`, with the BOD off, `SUT` 64 ms and the chip unlocked.

## The driver

- The converter is likely an MP3432, from a comment in the stock firmware and a BLF post. Nobody's shown a marking.
- From the MP3432 datasheet (Rev 1.0), EN turns on at 1.23 V typical and shuts down fully under 0.4 V, switching starts 180 µs after EN, it needs 2.7 V to start and 0.8 V once running, and it's rated 30 W average.
- PB2 is high impedance through reset and in programming mode (DS40002204A §16.3.1, §33.3.8.2). The MP3432 says "Do not leave EN floating" and doesn't pull it down itself, so a pull-down on the board is likely and unseen.
- Brightness is PWM on PB0, never the DAC. Confirmed from source. What turns the duty into a current is likely a filtered reference into an op-amp loop, and nobody's traced it.
- The stock firmware runs TCA0 dual-slope at 10 MHz with TOP 255 (about 19.6 kHz) and seven bits of delta-sigma, so levels run 0 to 32640 with a floor of 20. Confirmed from source.
- 1Lumen measured [11.30 A at the tail](https://1lumen.com/review/wurkkos-ts26/) at turbo start, and 52 °C at the head.
- On this torch a duty of 10 of 255 draws 0.33 A.
- On this torch duties of 24 to 37 of 32640 flicker slightly, and a turn-on flashes, dips and rises over several hundred milliseconds. A duty of 60, with PB0's PWM already running while PB2 is still low, looks steady with no bounce, by eye.

## Battery and temperature

- The cell is Wurkkos's unprotected 21700.
- PA1 reads the cell through a divider against the 1.1 V reference. The stock firmware's slope for this board is 1094, an effective ratio of about 4.03, calibrated on one sample. The resistors are unknown.
- The stock low-voltage threshold is 2.90 V. Confirmed.
- The temperature sensor is inside the MCU. The stock firmware limits at 50 °C and stops stepping down at its level 50. Confirmed. How the die tracks the head is unknown.
- The stock firmware draws 0.12 mA off, on this torch.

## The flashing pads

Three round bare pads in a row at the edge of the driver's battery side, at about 4 o'clock with the `TS26-A-S5` marking at 12. The silkscreen reads `R - +`. On this torch `-` is ground, `+` the supply and `R` UPDI on PA0. With the cell out, only `-` is continuous with battery negative and every pair of pads reads in the megohms. The pitch is likely 1.27 mm and unmeasured.

A supply on `+` reaches the boost converter's input, so the converter can run from the pad, and PB2 floats in programming mode unless the board holds it. On this torch the emitters blinked once as a supply landed on `+` with the cell out, and stayed dark in programming mode.
