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

## The driver

- The converter is likely an MP3432, from a comment in the stock firmware and a BLF post. Nobody's shown a marking.
- From the MP3432 datasheet (Rev 1.0), EN turns on at 1.23 V typical and shuts down fully under 0.4 V, switching starts 180 µs after EN, it needs 2.7 V to start and 0.8 V once running, and it's rated 30 W average.
- PB2 is high impedance through reset and in programming mode (DS40002204A §16.3.1, §33.3.8.2). The MP3432 says "Do not leave EN floating" and doesn't pull it down itself, so a pull-down on the board is likely and unseen.
- Brightness is PWM on PB0, never the DAC. Confirmed from source. What turns the duty into a current is likely a filtered reference into an op-amp loop, and nobody's traced it.
- The stock firmware runs TCA0 dual-slope at 10 MHz with TOP 255 (about 19.6 kHz) and seven bits of delta-sigma, so levels run 0 to 32640 with a floor of 20. Confirmed from source.
- 1Lumen measured [11.30 A at the tail](https://1lumen.com/review/wurkkos-ts26/) at turbo start, and 52 °C at the head.

## Battery and temperature

- The cell is Wurkkos's unprotected 21700.
- PA1 reads the cell through a divider against the 1.1 V reference. The stock firmware's slope for this board is 1094, an effective ratio of about 4.03, calibrated on one sample. The resistors are unknown.
- The stock low-voltage threshold is 2.90 V. Confirmed.
- The temperature sensor is inside the MCU. The stock firmware limits at 50 °C and stops stepping down at its level 50. Confirmed. How the die tracks the head is unknown.

## The flashing pads

Three round bare pads in a row at the edge of the driver's battery side, at about 4 o'clock with the `TS26-A-S5` marking at 12. The silkscreen reads `R - +`. On this torch `-` is ground, `+` the supply and `R` UPDI on PA0. Pad order differs between lights, so measure another board before trusting its labels.

The JLHawaii808 kit's 1.27 mm probe fits, so 1.27 mm is likely, and nobody has measured it. An earlier torch's pads measured about 1.75 mm.

The programmer's supply on `+` reaches the boost converter's input, so the converter can run from the programmer, and PB2 floats for the whole of a backup or a flash unless the board holds it. Only a plain UPDI adapter ever goes on these pads.

## Seen on this torch

The test builds are on branches `build-t0` (`9d3b7dc`), `build-t2`, `build-t3`, `build-t4`, `build-t4-main` and `build-t5`.

- 2026-11-12. Cell 4.138 V out of the light. Stock drew 0.12 mA off.
- 2026-11-12. Cell out, only `-` continuous with battery negative, and every pair of pads in the megohms.
- 2026-11-16. `found tinyAVR P:0D:0-3` on the pads, with one blink from the emitters as the probe landed, cell out. Dark in programming mode.
- 2026-11-16. Stock backed up to `backup/torch`, serial `30 4F 31 4B 50 34 4C 00 17 2F`. Fuses `00 00 02 FF 00 F6 07 00 00 FF C5`, the factory defaults, with the BOD off, `SUT` 64 ms and the chip unlocked.
- 2026-11-16. T0 flashed and ran. Of about nine power-ups, one gave a bright red aux that T0 never drives and one a brief bright flash on a bouncing contact. It drew 0.33 A lit at duty 10 of 255, and 2.96 mA off, awake.
- 2026-11-16. The full firmware (`af3aef9`) gave ten clean power-ups, clicks, ramps and lockout as specified, and an hour lit at a low level. Off with blue, 0.58 mA.
- 2026-11-16 to 2026-11-20. T4, which powers down after a watchdog feed, drew 0.12 mA locked and dark and 0.14 mA with dim blue, and ran for several days with no reset or bite.
- 2026-11-20. The 1 Hz rest (`2e32708`) woke on every press with no reset or fault, at 0.13 mA off.
- 2026-11-20. Floors of level 5, 11 and 13 (24 to 37 of 32640) flickered slightly, and every turn-on flashed, dipped and rose over several hundred milliseconds. `9cf29a5`, with a floor of level 17 (60) and PB0's PWM running with PB2 low during the press, looked steady with no bounce, by eye.
- On an ATtiny1616 breakout, the clock ran about 0.9 % fast and a deliberate hang made the watchdog bite. Neither has been seen on this torch.

Not measured on this torch are PB2 to ground, the pad pitch, the PWM frequency, the battery reading against a meter, current against duty, anything thermal, the low-voltage steps and the top of the ramp.

## Open questions

- Which chip is the boost converter, and what turns PB0's PWM into current?
- What are the divider resistors on PA1, and how good is the 1094 calibration near 3 V and under load?
- What holds PB2 low through reset and in programming mode?
- What feeds the MCU's VCC, and does it hold 2.7 V with the cell at 2.80 V?
- Is the low-end flicker the driver, the delta-sigma, or both?
