# Hardware

What's inside a TS26 (model 0721), and how sure the docs are about each bit. "Confirmed" means a primary source (Microchip's files, the stock firmware's source, a datasheet, or someone who opened one up) says so. "Likely" means it follows from a sibling light, a reviewer's guess or a convention and nobody's contradicted it. No teardown or driver photos of the TS26 have been found, so everything about the board is, at best, what the stock firmware's build for it implies.

Facts from the stock firmware come from its source, as it stood in November 2026. Anything it sets in a register could be checked against the backup of a stock torch's flash, and nobody's done that yet. What only its comments say, such as resistor values and calibration history, can't be.

## Spotting a model 0721

A version check from off (15 or more clicks) blinks the build date and then the model number, which is 0721. Reviewers report "0721-2025" ([1Lumen](https://1lumen.com/review/wurkkos-ts26/), [EDC Tips](https://www.edctips.com/547318/wurkkos-ts26-review-with-runtime-graphs/)). Confirmed as what two reviewers saw.

The stock firmware's model list maps 0721 to the TS26 on an ATtiny1616. It went by the TS25 boost name until June 2025, renamed "because the product name changed upstream". Confirmed.

Don't mix it up with the TS26S. Wurkkos calls that an interim model, released while the TS26 was still being refined ([Wurkkos blog, 11 March 2026](https://wurkkos.com/blog/detail/ts26-anduril-%E2%80%93-discover-the-story)). It has the same body and four 519A emitters, with a simpler UI of its own ([BLF review](https://budgetlightforum.com/t/review-wurkkos-ts26s-4x-nichia-519a-and-boost-driver/224330)). 1Lumen says the TS26's driver is "probably carried over" from the TS26S and modified. The TS26S isn't a 0721, so treat anything measured on one as a hint about the hardware only.

The product page lists two versions, 519A 5000 K high CRI and TN3535 6000 K, and can't decide between 3,400 and 3,470 lumens ([wurkkos.com](https://wurkkos.com/products/ts26-anduril-edc-flashlight-reverse-charging)). The 519A version is rated about 1,000 lm lower at turbo (2,500 against 3,400) per [EDC Tips](https://www.edctips.com/578039/wurkkos-ts26-519a-now-with-better-leds/), and that reviewer says the driver "seems to be the same" ([BLF](https://budgetlightforum.com/t/wurkkos-ts26-519a-now-with-better-leds/234755)). Likely, since nobody's opened both.

## The driver

The stock firmware's hardware definition for the TS26 says "Boost driver based on MP3432 and attiny1616 ... hardware version : 2.0+, compatible with BST20-FWxA v1.0 (no button LED A)". Confirmed as what the stock build says, in the sense that Wurkkos ships that build. The MP3432 itself is likely. It rests on that comment and a BLF poster's "the same MP34XX boost converter that maxes out ~40W" ([crbnfbr, 14 January 2026](https://budgetlightforum.com/t/wurkkos-ts26-update-ts26-anduril-version-update/230944?page=6)). Nobody's shown a chip marking.

| Pin | Job | Notes |
|---|---|---|
| PA0 | UPDI | The programming pad |
| PA1 | Battery sense | Through a divider, ADC AIN1 |
| PA5 | Side switch | Active low, internal pull-up. The stock firmware also takes an interrupt on both edges; this firmware reads the pin every tick and takes none |
| PB0 | Brightness | TCA0 WO0 (CMP0), PWM into the current control |
| PB2 | Boost enable | Output, high runs the boost converter |
| PC1, PC2, PC3 | Aux blue, green, red | Plain on/off GPIO |
| VCC | MCU supply | Regulated, not the cell |

PA2 to PA4, PA6, PA7, PB1, PB3 to PB5 and PC0 are unused, and get the pull-up. PA6 is the ATtiny1616's DAC0 pin (the ATDF's `signal function="DAC0" ... pad="PA6"`), unused here. Confirmed. A June 2026 change to the stock firmware fixed the switch's pin control register from PA2's to PA5's with "no functional change", so the switch has always been PA5.

The stock firmware's setup leaves PA1 and PB2 at their reset values (the pull-up lines for both are commented out) and makes PB2 an output with `VPORTB.DIR`, so it drives the reset value of `OUT`, low. Confirmed. This firmware writes `OUT` to zero first and then `DIR`, so it doesn't count on having had a reset. It also turns PA1's digital input buffer off, as the datasheet asks for a pin the ADC reads (DS40002204A §30.3.1.1), where the stock firmware leaves it on. Nobody's compared readings on a real TS26 either way. Before that, during reset, and for as long as a programmer holds the chip in programming mode, PB2 is a high-impedance input, and the MP3432 datasheet says "Do not leave EN floating". So a pull-down on the board is likely, but nobody's seen one.

### PWM, not a DAC

The header comment mentions "DAC control", but that's borrowed from the stock firmware's definition for thefreeman's FWAA boost driver, which really does use the DAC on PA6 with an op-amp and sense resistor. The TS26 code only ever writes `TCA0.SINGLE.CMP0BUF`, never DAC0. Confirmed from source.

The MP3432 has no dimming input. FB takes a resistor divider from VOUT against a 1.0 V reference, and the only logic inputs are EN and MODE ([datasheet](https://www.monolithicpower.cn/cn/documentview/productdocument/index/version/2/document_type/Datasheet/lang/en/sku/MP3432/document_id/4864/), Rev 1.0, pin functions). So something outside the chip turns PB0's duty into a current, most likely a filtered reference into an op-amp loop as on the freeman board. That's an inference, so likely. Nobody's traced the board.

Here's what the stock firmware does with PB0, all confirmed from its source.

- TCA0 in dual-slope mode off the undivided 10 MHz clock, TOP fixed at 255, so 10 MHz / (2 × 255), about 19.6 kHz.
- Seven bits of delta-sigma on top, stepped in the TCA0 overflow interrupt once a PWM cycle, so ramp values are 15-bit (0 to 32640, which is 255 << 7). The pattern repeats every 128 cycles, a dither at about 153 Hz. That only makes sense if something downstream filters the PWM, which is a second reason to think it's a filtered reference (likely).
- 150 levels, from 20 to 32640 in those units. ToyKeeper wrote in January 2026 that "It won't light up below DSM 10/32640, and is really unstable ... I raised the floor to 20/32640, which is as low as it can go. So, no real 'moon' mode on this light." Confirmed as what she says about her sample. 1Lumen measured level 1 at 7 lm on a TN3535 light.
- Party strobe is slowed because "this driver can't pulse for less than 2ms". The datasheet gives 180 µs typical from EN to switching, so the rest of the 2 ms is probably the analogue side settling. A guess.

Reviewers saw candle-like flicker at the lowest levels ([EDC Tips](https://www.edctips.com/552908/review-new-wurkkos-ts26-nice-enthusiast-flashlight-but-lacking-enthusiast-features/), and 1/150 to 5/150 in the [BLF 519A review](https://budgetlightforum.com/t/wurkkos-ts26-519a-now-with-better-leds/234755)), and a Redditor said it's "only present on some units" (secondhand). The stock firmware raised this light's default smooth floor to 5/150 "by request". That level is 36 in delta-sigma units, since the floor is level 5, the fifth entry in the TS26's level table is 36, and levels count from 1. Confirmed from source. It's the figure `ui::FLOOR` is picked to clear. Likely the driver struggling with very small currents. Unmeasured.

This firmware's ramp is its own curve, not the stock firmware's, from that 20 up to 32640, with the UI working between levels 13 and 120. A fresh torch comes on at level 52, a duty of about 10 of 255, well clear of the floor. Level 13 is about 37 in those units, which should light but is close enough to the bottom that reviewers' flicker could show. Level 120 is about half of full duty. None of it's been measured.

### Turning on and off

To turn on, the stock firmware writes the compare value, enables the overflow interrupt, resets the counter if it's coming from off, then sets PB2 high, with no delay in between. To turn off it disables the interrupt, zeroes the compare value and the counter, then clears PB2. Confirmed as the stock firmware's order, though not as the only safe one.

This firmware turns on in the same order (duty, then PB2). It turns off the other way round, PB2 first, then the duty. Because `CMP0BUF` only lands at the next BOTTOM, the pin can run for up to one PWM period (about 51 µs) with the boost enabled at duty 0 on the way up. That's inside the 180 µs the converter takes to start switching, so it's probably harmless. Not checked on a real light.

PB0 only belongs to TCA0 while the light's on. `hw::set_output` sets CMP0EN as it turns on and clears it as it turns off, handing the pin to PORT, whose OUT is 0. The datasheet doesn't say what level a waveform output holds in power-down with TCA0 stopped, so nothing depends on it. Going off also writes CMP0 directly, so power-down can follow on the same pass. The datasheet doesn't say whether that clears the waveform output before BOTTOM, so PB0 could go high for up to one cycle at the next turn-on, also inside that 180 µs. That's reasoning, not a measurement.

From the MP3432 datasheet (Rev 1.0, confirmed for the chip), EN turns on at 1.23 V typical, switching starts 180 µs after EN, shutdown current is 2 µA at most, it needs 2.7 V on VIN to start (0.8 V once it's running), switches at 600 kHz, and is rated for 30 W average or 40 W peak from 3.3 V, with the peak switching current set by a resistor (10 A typical with 36 kΩ). The 2.7 V start-up is worth remembering against a 2.8 to 2.9 V cut-off. Nobody knows this board's ILIM resistor or output voltage.

1Lumen measured 11.30 A at the tail at turbo start (3,476 lm at turn-on, 3,318 lm at 30 s). That's about 40 W from the cell, near the part's peak rating (arithmetic, so likely). A TS26S drew 8.55 A ([SammysHP](https://budgetlightforum.com/t/review-wurkkos-ts26s-4x-nichia-519a-and-boost-driver/224330)). 1Lumen saw turbo fall to about 600 lm after about ten minutes and hold that for about two hours before low-voltage protection. One BLF poster says the staircase is "hardcoded in the controller by Wurkkos" and another says it's thermal ([thread, page 3](https://budgetlightforum.com/t/wurkkos-ts26-update-ts26-anduril-version-update/230944?page=3)). Nothing in the stock firmware's TS26 build has a timed step-down, so thermal looks more likely. Unresolved.

## Battery measurement

The TS26 doesn't read its own supply. The hardware definition says "use a dedicated pin, not VCC, because VCC input is regulated". Confirmed. The stock firmware reads PA1 (AIN1) through a divider against the 1.1 V internal reference, accumulated 4×, with `INITDLY` DLY16, free-running. Here's how its ATtiny1616 code turns that into volts.

- raw is the accumulated 10-bit result shifted left by 4
- the voltage in 1/6400 V is raw × 473 / slope + offset × 64, where a slope of 1024 assumes the cell is 4.3 times the divider's output
- this board sets the slope to 1094 ("default = 1024, higher = lower voltage") and the offset to 0

So the effective ratio is 4.3 × 1024 / 1094, about 4.03, and the ADC tops out with the cell at about 1.1 × 4.03 = 4.43 V. The sums are confirmed (arithmetic); the ratio is only as good as one sample's calibration.

The stock firmware's history has two stories for the resistors. Until August 2026 the definition said "Resistors are 330k and 100k" (4.3) alongside a raw value for 4.40 V that works out to 4.0. The later slope is the better bet, since it came from a measured sample ("calibrated for latest TS26 sample", January 2026) and agrees with the 4.4 V figure; the 330k/100k comment looks like a template from the prototypes. 100k and 33k would give 4.03, 300k and 100k 4.0. Nobody states the real values. Likely.

Either way the source impedance at PA1 is tens of kilohms, which matters for the ADC's sample time. The ADC runs at 625 kHz and samples for 22 clocks, 35 µs, which the temperature sensor's 32 µs sets. With the worst of those stories, 77 kΩ plus the ADC's own 14 kΩ into 5 pF (DS40002204A table 36-25), settling to half a bit of 10 takes about 3.5 µs, so there's ten times what it needs. That's sums from the datasheet, and unmeasured.

No TS26 reading has been compared with a multimeter in public, and Wurkkos's blog mentions "inaccurate battery readings" as a problem during development.

### Low voltage

The TS26's build doesn't override the stock firmware's low-voltage threshold anywhere, so it's the default of 2.90 V. Confirmed. Wurkkos quotes a cut-off "below 2.8 V", and 1Lumen saw output step down "until about 2.9 V". Trust the firmware and treat 2.8 V as marketing. Likely.

The cell is Wurkkos's own unprotected 21700, 5,000 mAh, flat top, quoted at 10 A continuous and 15 A peak (claimed, not measured). 11.3 A at turbo is over the continuous figure. Protected cells don't fit.

## Temperature

The sensor inside the MCU, on the driver.

- The stock firmware's Wurkkos builds raise its default limit from 45 °C to 50 °C, "higher temperature limit because attiny1616 is more accurate". Confirmed.
- Its TS26 build stops stepping down at level 50 (of 150), and steps faster above 120. Confirmed.
- This firmware borrows that 50 for its thermal floor and for the cap after a failed check (`thermal::FLOOR`, `limits::FAULT_CEILING`), but on its own curve, where level 50 is about 3.6% of full duty. That isn't the stock firmware's level 50, and nobody's measured how bright either is.
- The sensor's calibration offset is 0 and not overridden. 1Lumen calls the sensor "precalibrated to within ±1 to 2 °C" without saying how they know.
- 1Lumen's thermocouple on the head peaked at 52 °C. One sample.

## The flashing pads

Wurkkos says the board has them ("Yes, the pads on the PCB does have", 26 November 2025, [BLF](https://budgetlightforum.com/t/wurkkos-ts26-update-ts26-anduril-version-update/230944)), and the product page says "Flashing Pads Available". 1Lumen calls them a "3-pin UPDI pattern" and EDC Tips counts "three flashing pads on the driver". The driver is glued in. Confirmed that there are three.

Nobody gives their position, labels, order or pitch, and the [reflash reference](https://budgetlightforum.com/t/anduril-flashlight-reflash-reference-updated-for-2024/216942) has no TS26 entry. The [Unofficial Guide](https://anduril.click/drivers/updi.html) warns that pad order differs between lights. So an R, G, V layout borrowed from another light is a guess here, not a fact. Don't assume a pitch either. An earlier torch's pads measured about 1.75 mm, not the 1.27 mm the standard layout has and had been written down for it, so the standard is no guide. A 1.27 mm probe might fit, but until someone measures the pads that's a guess.

Before pressing a programmer onto them, take the cell out, measure the pitch, and check with a multimeter which pad is continuous with battery negative. That only names ground. Which of the other two is the supply and which is UPDI is unknown as well, and row 1.5 of [breakout.md](breakout.md#stage-1-the-pads-and-stock) tells them apart with the meter. The prediction it works from is reasoning from the datasheet, and unmeasured. The supply pad should read low against ground and climb as the board's capacitors charge, and the UPDI pad, with only the chip's pin on it, should read open or very high from the start. VCC on this board comes from a regulator nobody can identify, and nobody knows what feeding it from a programmer does. Only a plain UPDI adapter goes on these pads, never the 12 V HV Friend ([flashing.md](flashing.md#only-the-plain-adapter-goes-on-this-torch)).

What the programmer's supply reaches is unknown too. With the cell out it feeds the regulator's output, and if that gets back to the cell rail, the MP3432 has an input to start from (2.7 V, as above). The MCU only holds the enable low while firmware's running. PB2 is tri-stated after a reset (DS40002204A §16.3.1), and in programming mode the CPU isn't running to drive it (§33.3.8.2), so it floats for the whole of a backup or a flash unless the board holds it. That's a reading of those two sections together. The stock firmware also raises it every time it starts, blinking the main LEDs once, and the TS26's build doesn't turn that blink off. Confirmed from source. So a programmer landing on a stock light starts the stock firmware, which asks the converter for a blink. Whether anything lights is unmeasured, and [breakout.md](breakout.md#what-the-adapter-powers) makes the first contact in the dark because of it.

A search on 2026-11-09 of the [BLF TS26 thread](https://budgetlightforum.com/t/wurkkos-ts26-update-ts26-anduril-version-update/230944), the [reflash reference](https://budgetlightforum.com/t/anduril-flashlight-reflash-reference-updated-for-2024/216942), [thefreeman's programmer thread](https://budgetlightforum.com/t/wts-usb-updi-programmers-for-attiny1616-avr32dd20-drivers-sofirn-wurkkos-fireflylite-emisar/71459), [anduril.click](https://anduril.click/drivers/updi.html) and [JLHawaii808's listing](https://jlhawaii808.com/products/updi-programmer-driver-reflashling-kit) found no public report of anyone reflashing a TS26 through its pads. So there's no pad order, pitch or adapter voltage to borrow, and no report either way of a programmer back-feeding a boost driver. JLHawaii808's listing doesn't name the TS26. That's a gap in what's been published, and it says nothing about the board.

## The MCU

An ATtiny1616, with flash mapped into data space at 0x8000, two-word vectors, 16-bit registers written low byte first and `CPU.CCP` guarding clock changes. Never write its fuses. The rest is particular to this board.

- The stock firmware runs the clock at 10 MHz (the 20 MHz oscillator divided by 2). Confirmed. VCC is regulated here, and the PWM frequency hangs off the clock.
- The battery is read on AIN1 against the 1.1 V reference.
- TCA0 stays at TOP 255; no variable TOP.

Register addresses and bit values in `src/regs.rs` are generated from Microchip's ATDF in [`vendor/`](../vendor/README.md) at build time.

## The rest of the light

Four emitters, either Nichia 519A (5000 K, high CRI) or LatticePower TN3535 (1Lumen measured 6,422 K and Ra 69.7 at turbo). IPX8, magnetic tail, USB-C under a rubber flap, reverse polarity protection (reviewers' statements).

Eight RGB aux emitters sit on a separate board behind the optic (EDC Tips), on PC1 to PC3 as three on/off channels ("aux RGB passive" in the hardware definition). Likely. With aux on, 1Lumen measured 0.22 mA (low, red), 5.4 mA (high, red) and 7.4 mA (high, purple). Nobody's published the drain with the aux off.

The button LED is separate. It's green while charging, solid green when full and blue for a few seconds after unplugging (EDC Tips). The hardware definition gives it no MCU pin, so the charger probably drives it, and "no button LED A" in the header probably means the same. Likely.

USB-C charging measured at about 13 W (5 V, 2.6 A), 2 h 03 min, 4,265 mAh in and 4.17 V at the end (1Lumen). Nobody names the charger chip. Reverse charging is claimed at 5 V 3 A, and [LMP_BR](https://budgetlightforum.com/t/wurkkos-ts26-519a-now-with-better-leds/234755) measured it "limited at 5V 1A". The measurement is the better bet.

The stock firmware's history for this light (first under the TS25 boost name, then the TS26) has a first version "made from spec" in December 2023, prototype calibrations in March 2024, August 2024 and February 2025, the rename in June 2025, the floor changes above and an aux LED rewrite in August 2026. Wurkkos says it launched on 12 March 2026. Nothing says the shipping hardware changed after that, and nothing says it didn't.

## Open questions

- Which chip is the boost converter? Is it really an MP3432?
- What turns PB0's PWM into LED current, and with what filter and scaling?
- What are the real divider resistors on PA1, and is there a diode in the path?
- What holds PB2 low while the MCU is in reset or being programmed?
- Where are the pads, how are they labelled, and in what order and pitch? What does VCC do with the cell out and a programmer on, and does the programmer's supply reach the cell rail?
- Which charger chip, and does it have a status pin the MCU could read?
- How good is the 1094 calibration on other lights, near 3 V and under turbo load?
- Is the low-end flicker the driver, the delta-sigma, or both, and is it only some lights?
- Is the turbo step-down thermal, or a timer?
