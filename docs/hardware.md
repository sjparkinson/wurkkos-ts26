# Hardware

What's known about a Wurkkos TS26 (model 0721), and how sure each fact is.

- **Confirmed** means a primary source says so (Microchip's files, a datasheet, the stock firmware's source, or someone who opened one up).
- **Likely** means it follows from a sibling light, a reviewer or a convention and nothing contradicts it.
- **On this torch** means it was read or seen on this project's TS26.

No published teardown or driver photo of a TS26 has been found. Facts from the stock firmware come from its source as it stood in November 2026, and nobody has checked what it sets in a register against the backup of this torch's flash. What only its comments say (resistor values, calibration history) can't be checked from here.

## Spotting a model 0721

A version check from off (15 or more clicks) blinks the build date and then the model number. Reviewers report "0721-2025" ([1Lumen](https://1lumen.com/review/wurkkos-ts26/), [EDC Tips](https://www.edctips.com/547318/wurkkos-ts26-review-with-runtime-graphs/)). The stock firmware maps 0721 to the TS26 on an ATtiny1616, under the TS25 boost name until June 2025. Confirmed.

The TS26S is an interim model with the same body and a UI of its own ([Wurkkos blog](https://wurkkos.com/blog/detail/ts26-anduril-%E2%80%93-discover-the-story), [BLF review](https://budgetlightforum.com/t/review-wurkkos-ts26s-4x-nichia-519a-and-boost-driver/224330)). It isn't a 0721, so anything measured on one is a hint at most. The 519A and TN3535 versions likely share a driver ([BLF](https://budgetlightforum.com/t/wurkkos-ts26-519a-now-with-better-leds/234755)).

## Pins

| Pin | Job | Notes |
|---|---|---|
| PA0 | UPDI | The programming pad |
| PA1 | Battery sense | Through a divider, ADC AIN1. Digital input buffer off (DS40002204A §30.3.1.1) |
| PA5 | Side switch | Active low, internal pull-up |
| PB0 | Brightness | TCA0 WO0 (CMP0), PWM into the current control |
| PB2 | Boost enable | High runs the boost converter. Never gets a pull-up |
| PC1, PC2, PC3 | Aux blue, green, red | Plain on/off GPIO |
| VCC | MCU supply | Regulated, not the cell |

PA2 to PA4, PA6, PA7, PB1, PB3 to PB5 and PC0 are unused and get the pull-up. Confirmed from the stock firmware's hardware definition, with the switch on PA5 since a June 2026 fix there. Blue on PC1 and red on PC3 are seen on this torch, green on PC2 isn't.

## The driver

The stock firmware's definition says "Boost driver based on MP3432 and attiny1616 ... hardware version : 2.0+, compatible with BST20-FWxA v1.0 (no button LED A)". The MP3432 is likely, resting on that comment and a BLF post. Nobody's shown a chip marking.

### The enable

During reset, and for as long as a programmer holds the chip in programming mode, PB2 is a high-impedance input (DS40002204A §16.3.1, §33.3.8.2). The MP3432 datasheet says "Do not leave EN floating" and gives EN's input current as 0 µA, so nothing inside the converter pulls it down. A pull-down on the board is likely and unseen. The stock firmware leaves PB2 at its reset value and makes it an output, so it drives low. This firmware writes `OUT` before `DIR`, so it doesn't count on a reset.

From the MP3432 datasheet (Rev 1.0, confirmed for the chip), EN turns on at 1.23 V typical, switching starts 180 µs after EN, a full shutdown needs EN under 0.4 V, it needs 2.7 V on VIN to start and 0.8 V once running, and it's rated 30 W average or 40 W peak. Whether the output is cut off from the input with EN low isn't stated. This board's ILIM resistor and output voltage are unknown.

The stock firmware turns on with the duty, then PB2, and off with the duty zeroed, then PB2 cleared. This firmware turns off the other way, PB2 first. `CMP0BUF` only lands at the next BOTTOM, so at turn-on PB2 can be high at duty 0 for one PWM period (about 51 µs), inside the 180 µs start-up. Reasoning, not measured.

### PWM, not a DAC

The TS26 code only ever writes `TCA0.SINGLE.CMP0BUF`, never DAC0. Confirmed from source. The MP3432 has no dimming input, so something outside it turns PB0's duty into a current, most likely a filtered reference into an op-amp loop. Likely, and nobody's traced the board.

The stock firmware runs TCA0 dual-slope off 10 MHz with TOP 255, about 19.6 kHz, with seven bits of delta-sigma on top stepped once a PWM cycle, so levels are 15-bit (0 to 32640). Its floor is 20 of 32640 ("It won't light up below DSM 10/32640"), and its default smooth floor is level 5, which is 36 in those units, "by request". Confirmed from source. Reviewers saw flicker at the lowest levels, likely the driver at very small currents. Unmeasured.

This firmware's ramp is its own curve over the same range, with the UI between levels 13 and 150. Level 13 is 32 in those units, clear of the stock floor and under the stock smooth floor. Levels 5 (24) and 11 (30) flickered slightly when steady and bounced as 1H lit them from off. Earlier builds had a floor of level 13, reached only by ramping down, with no flicker reported. Whether 1H from off bounces at level 13 is unmeasured.

1Lumen measured 11.30 A at the tail at turbo start, about 40 W from the cell, and turbo falling to about 600 lm after ten minutes. The stock TS26 build has no timed step-down, so that's likely thermal.

## Battery measurement

VCC is regulated, so the stock firmware reads PA1 (AIN1) through a divider against the 1.1 V reference. Its voltage sum uses a slope of 1024 for a 4.3 ratio, and this board sets 1094, an effective ratio of about 4.03, which puts the ADC's top at about 4.43 V. Confirmed as sums. The slope came from a "latest TS26 sample" in January 2026, and an older comment says 330k and 100k (4.3). The real resistors are unknown, so the ratio is likely.

The source impedance is tens of kilohms. At 625 kHz and 22 clocks of sampling (35 µs) there's about ten times what settling to half a bit needs (DS40002204A table 36-25). Sums, unmeasured.

The stock low-voltage threshold for this build is its default 2.90 V. Confirmed. The cell is Wurkkos's unprotected 21700, quoted at 10 A continuous and 15 A peak.

## Temperature

The sensor is inside the MCU. The stock firmware's Wurkkos builds set the limit to 50 °C, and its TS26 build stops stepping down at level 50. Confirmed. `thermal::FLOOR` borrows that 50 on this firmware's curve, where it's about 3.6 % of full duty. How the die tracks the head is unknown. 1Lumen's thermocouple on the head peaked at 52 °C.

## The flashing pads

Three round bare pads in a row at the edge of the driver's battery side, between the spring and the brass ring, at about 4 o'clock with the `TS26-A-S5` marking at 12. The silkscreen beside them reads `R - +`, with `R` nearest the `20251008` marking. On this torch the meter and the programmer agree that `-` is ground, `+` the supply and `R` UPDI on PA0. Pad order differs between lights ([anduril.click](https://anduril.click/drivers/updi.html)), so measure another board before trusting its labels.

The pitch fits the JLHawaii808 kit's probe, which is made for 1.27 mm, so 1.27 mm is likely. Nobody has measured it in millimetres. An earlier torch's pads measured about 1.75 mm, so check the fit before trusting a probe.

The programmer's supply on `+` reaches the boost converter's input, since the stock firmware's start-up blink lit the emitters from it with the cell out. So the converter can run from the programmer, and PB2 floats for the whole of a backup or a flash unless the board holds it. Nothing lit while the chip sat in programming mode, which fits a pull-down without proving it. The spring is likely the cell's positive contact, so a probe that slips onto it from `+` feeds the cell rail directly (reasoning).

PA0 is the one pin rated to 13 V (DS40002204A table 36-1), but only a plain UPDI adapter ever goes on these pads, never a high-voltage one, since nobody knows what else sits on them.

## The MCU

An ATtiny1616, with flash mapped into data space at 0x8000, two-word vectors, 16-bit registers written low byte first and `CPU.CCP` guarding clock changes. The stock firmware runs it at 10 MHz (20 MHz divided by 2), and so does this firmware. Register constants come from the ATDF in [`vendor/`](../vendor/README.md). Never write its fuses.

## The rest of the light

Four emitters, Nichia 519A or LatticePower TN3535. Eight RGB aux emitters on a separate board, on PC1 to PC3 ("aux RGB passive"). With aux on, 1Lumen measured 0.22 mA (low, red), 5.4 mA (high, red) and 7.4 mA (high, purple). The button LED has no MCU pin, so the charger likely drives it. USB-C charging measured about 13 W, and reverse charging "limited at 5V 1A".

The driver carries `TS26-A-S5` and `20251008` on its battery side. On this torch. The second is likely a date, 8 October 2025.

## Seen on this torch

The T0 to T5 test builds are kept on branches `build-t0` (`9d3b7dc`), `build-t2`, `build-t3`, `build-t4`, `build-t4-main` (T4 on main, as it ran on the torch) and `build-t5`. Before the torch, the programmer, T0 and the full firmware ran on an ATtiny1616 breakout, which is a different chip and proves nothing about this board.

- 2026-11-12. Cell 4.138 V out of the light. On stock, 0.12 mA at the tail off, and 0.65 A on at an unknown level.
- 2026-11-12. Pads photographed and their `R - +` silkscreen read. The kit's probe fits them, no figure in millimetres.
- 2026-11-12. Cell out, only `-` continuous with battery negative. `+` to `-` climbed from about 1.8 MΩ to 2 MΩ as capacitors charged, and `R` to `-` read about 20 MΩ from the start, so `+` is supply and `R` UPDI. Pairs read in the megohms, red on `-` to `+` about 50 kΩ. No short. Diode mode from `+` to `R` 1.872 V and `R` to `+` 2.890 V, neither a junction drop, which fits PA0 having no clamp to the supply (an inference).
- 2026-11-16. The kit, in the air, said "no reply from the chip". On the pads, `found tinyAVR P:0D:0-3`, with one blink from the emitters as the probe landed (cell out), the stock firmware's start-up blink. "Nothing echoed back" hasn't been seen on this torch.
- 2026-11-16. Stock backed up to `backup/torch` (gitignored), serial `30 4F 31 4B 50 34 4C 00 17 2F`. Dark while in programming mode, and a second blink likely as `backup` ended.
- 2026-11-16. Fuses `00 00 02 FF 00 F6 07 00 00 FF C5` (`WDTCFG` to `LOCKBIT`), the factory defaults. `selftest::Fuses::ok` passes, BOD off, `SUT` 64 ms, unlocked.
- 2026-11-16. T0 flashed (628 bytes, hex SHA-1 `c16cdc85d0606c2c24453b481dad4553e74aa6af`) and verified, and the chip still talked after it.
- 2026-11-16. T0 on the light. Of about nine power-ups, one gave a bright red aux that T0 never drives, and one a brief bright flash from the main emitters on a bouncing contact as the cap went on. The rest were dark. Likely a power-up on a bouncing supply with the BOD off, unconfirmed. Clicks were clean, with no flash at turn-on, at duty 10 of 255.
- 2026-11-16. T0 drew 0.33 A at the tail lit at duty 10 of 255, and 2.96 mA off (awake, since T0 never sleeps). Ten minutes lit, steady, the head barely warm. If duty maps near linearly to current, 3 A comes at about 35 % duty and level 120 is over 4 A, so likely, and unmeasured.
- 2026-11-16. The full firmware flashed (`af3aef9`). Ten power-ups with no flash, aux dim blue each time. Clicks, ramps and lockout as specified. Off at the tail 0.58 mA with blue. Off dim blue, lockout red, nothing while lit. An hour lit at a low level, stable, so the cell reads as valid here.
- 2026-11-16. T4, which powers down on the tick after a watchdog feed, flashed. Off at the tail, meter and range not noted, 0.12 mA locked with the aux dark and 0.14 mA with dim blue, so blue costs about 0.02 mA.
- 2026-11-16. `25f5695` flashed. The 220 ms click window, the ramp turning round on each 1H and lockout's five seconds of red all worked.
- 2026-11-16 to 2026-11-20. T4 left on the torch for several days with no reset or bite reported.
- 2026-11-17. Main (`1124a7b`) off overnight with dim blue, then 1C lit at once.
- 2026-11-20. `2e32708` flashed, the first build with the 1 Hz rest. A peek caught it resting with the PIT at 1 Hz, the 8 s watchdog and PB2 low. Presses from rest worked with no reset or fault seen. Off at the tail 0.13 mA, aux state not noted, against T4's 0.12 to 0.14 mA, so any saving is within the meter's reading.
- On the breakout, T0's PWM read 19.79 to 19.80 kHz against 19.61 kHz, so its clock ran about 0.9 % fast, and a deliberate hang (T3) made the watchdog bite. Neither has been seen on this torch.

Not measured on this torch yet are PB2 to ground, the pad pitch, the PWM frequency, the battery reading against a meter, current against duty, anything thermal, the low-voltage steps and the top of the ramp.

## Open questions

- Which chip is the boost converter, and what turns PB0's PWM into current?
- What are the divider resistors on PA1, and how good is the 1094 calibration near 3 V and under load?
- What holds PB2 low through reset and in programming mode?
- Is the pitch exactly 1.27 mm, and what does the programmer's supply reach besides the converter?
- Which charger chip, and does it have a status pin?
- Is the low-end flicker the driver, the delta-sigma, or both?
