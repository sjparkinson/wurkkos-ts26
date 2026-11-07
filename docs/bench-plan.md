# Bench plan

Every hardware check the roadmap is waiting on, in the order to do them, for a bench with the torch, a Fluke 107, [JLHawaii808's 4.5 V UPDI kit](https://jlhawaii808.com/products/updi-programmer-driver-reflashling-kit) for the torch, and the [spare chip kit](#the-spare-chip-kit): an Adafruit ATtiny1616 breakout, an Adafruit High Voltage UPDI Friend, a breadboard, jumpers, tactile switches and blue LEDs. It's the TS21's plan, rewritten for this board. Nothing here has run on a TS26 yet, so treat each expected result as a prediction. [flashing.md](flashing.md) has how to use the programmer.

The order is the point of it. **The first flash on the torch is `first-light` (commit `9d3b7dc`), and the full firmware only goes on after stage 3 has passed.** The TS21 went dark straight after a verified flash of a much bigger start-up, and I was left unable to say whether that was firmware or hardware. A 628-byte image that clicks on and off keeps those two apart.

Each check says what it needs:

- **First light**: `first-light`, the firmware as of commit `9d3b7dc`.
- **Main**: the full firmware on main.
- **T1**, **T2**, **T3**, **T4**: a [test build](#test-builds).

The firmware passing the simulator doesn't change any of this. It's a mistake-finder written from the same reading of the datasheet as the firmware, and it doesn't model a chip reset or anything analogue, so none of its passes count as a result here.

Checks this kit can't do at all are at [the end](#what-this-kit-cant-do), with what would unlock them.

## What the kit can and can't see

The [Fluke 107](https://www.fluke.com/en-us/product/electrical-testing/digital-multimeters/fluke-107), from Fluke's spec table:

| It does | Which means |
|---|---|
| DC volts, 1 mV resolution on the 6 V range | Cell voltage, loaded or not, is the one thing it's really good at here |
| DC amps, 4 A and 10 A ranges only, 1 mA resolution, ±(1.5% + 3 counts) | Fine for the low half of the ramp. No mA or µA range, so nothing near a sleeping chip's current |
| Frequency, 10 Hz to 100 kHz | Reaches the 19.6 kHz PWM, if it can get a clean signal |
| Duty cycle, specified around 50/60 Hz only | Don't trust it on the PWM |
| Ohms, continuity, diode, hold | Pad and board checks with the cell out |
| 3 readings a second, no min/max | Can't catch anything shorter than about a second. Eyes are quicker than this meter |
| No temperature | Nothing here checks the sensor against a real temperature |
| 11 A fuse | Turbo drew 11.30 A in 1Lumen's test. See the rules below |

The programmer reads flash, EEPROM and fuses, and writes flash and EEPROM. It can't watch the firmware run. So apart from what the meter gets at the tail, what the firmware thinks is invisible unless a test build shows it with the light.

Your eyes are the third instrument. A dark room shows a millisecond flash from the boost converter far better than any of the above.

## The spare chip kit

The breakout is the same chip as the torch with nothing expensive attached, so everything risky happens there first. Its pins are also the one place the meter gets a clean logic signal: on the torch the PWM is buried in the driver, and here it's a header pin.

The first four rows are from the TS21's reading of [the breakout's schematic](https://github.com/adafruit/Adafruit-ATtinyx16-Breakouts-PCB), net by net. The PB2 and PC rows are my guess from Arduino pin numbers, so check all of them against the schematic and the silkscreen before wiring:

| Torch | Chip pin | Breakout header | What's different |
|---|---|---|---|
| PWM | PB0 | SCL | 10 kΩ pull-up to VCC, and a STEMMA QT socket |
| Side switch | PA5 | 1 | Nothing else on it |
| UPDI pad | PA0 | UPDI | 10 kΩ pull-up to VCC |
| Supply pad | VCC | VIN | VCC is VIN unless the jumper on the back's been changed. There's no VCC pin as such. 3V is a regulator output, so don't feed it |
| Boost enable | PB2 | TX (likely) | Check first. PB2 is USART0's default TX pin |
| Aux | PC1, PC2, PC3 | 11, 12, 13 (likely) | Check first |
| Ground | GND | GND | |

Wiring, with the Friend's slide switch on 3 V:

- Friend's UPDI, power and GND to the breakout's UPDI, VIN and GND.
- A tactile switch from pin 1 to GND. Mind which legs are joined: the 12 mm ones have four.
- A blue LED from SCL to GND and a second from PB2's pin to GND, cathodes to GND.

At 3.3 V a blue LED's forward voltage does most of the current limiting, so it's fine straight on a pin for this. At 5 V only the pin's own drive limits it, which could pass the 40 mA absolute maximum (DS40002204A table 36-1), so **keep the switch on 3 V** or add 1 kΩ in series. The header strips come with the board and need soldering.

What the breakout can't tell you: anything about the driver. There's no MP3432, no regulator and no emitters. PB0's pull-up means SCL goes high through a reset where the torch's pin floats, so a glow on that LED at power-on is expected here and says nothing about the torch. And it's a different chip from the torch's, possibly another silicon revision, so a pass here makes a torch pass likely without closing a roadmap item.

## The High Voltage UPDI Friend's 12 V pulse

Read this before plugging the Friend into anything. It's a serial UPDI adapter (CH340E, 1 kΩ from TX to RX, and it powers the target at 3.3 V or USB's 5 V), and it **never goes on the torch**, not for flashing and not for recovery.

Its [schematic](https://github.com/adafruit/Adafruit-High-Voltage-UPDI-Friend-PCB) has RTS# going through a 10 nF capacitor to the control pin of an analogue switch, which a 10 kΩ resistor holds high. High connects the UPDI line to the serial port. Low connects it to the 12 V rail, which is always up. So a falling edge on RTS# puts 12 V on UPDI for roughly 100 µs (my reading of the RC, not a spec), and no switch or jumper stops it. From the feedback resistors the rail should be about 12.4 V, if the MT3608's reference is the usual 0.6 V.

Opening a serial port normally asserts RTS, which is that falling edge, and `xtask/src/serial.rs` can't prevent it, because the kernel does it during `open`, before `stty` runs. So assume **every `cargo xtask` command fires one pulse as it starts.** Likely, not confirmed: the CH340 driver on macOS might behave differently, and the meter's too slow to see it.

What the datasheet says about that (DS40002204A):

- The pin takes it, on paper. The absolute maximum on RESET is 13 V (table 36-1), and detection is 11.5 to 12.5 V (table 36-11). But the same page warns that overshoot past 13 V can latch the pin's ESD protection on, and a 12 V driver can then push enough current through it to do permanent damage. Nothing here has measured any board's overshoot.
- It only describes the pulse as a way into UPDI from another pin function (§33.3.2.1.3). What it does to a chip already set to UPDI isn't stated.
- It's only rated for the RESET pin. On any other pin the limit is VDD + 0.5 V. **Never put the Friend's UPDI line on another pin**, on the breakout or anywhere else.

The TS21 was lost on 2026-11-06 after the Friend was hand-held on its pads, and it then measured about 2.5 Ω between all three. I think 12 V or a slipped probe did it, and that isn't proven. It doesn't need to be. There's a kit that does the job without 12 V, and on the TS26 nobody even knows what else is on the pads.

## Rules

- **Only the plain adapter on the torch.** The Friend never touches it, recovery included. Both adapters show up as CH340 `1a86:7523`, so nothing on screen tells you which you've got. Unplug the Friend and put it away before the head comes off, and look at the adapter before every command.
- **Nothing here writes fuses.** If a fuse looks wrong, stop and decide with the datasheet open. A bad `SYSCFG0` would need 12 V to undo, and that means the torch is gone.
- **Cell out before anything touches the pads.** The programmer powers the chip through the supply pad, and nobody knows yet what that does to the regulator. Check the ground pad is continuous with the battery negative first, every time you've had it apart.
- **Don't hold the probe by hand if you can avoid it.** Clamp it, or use something else that can't slip. A slip across pads this close together shorts them.
- **Measure before trusting a probe.** The JLHawaii808 probe is made for 1.27 mm pads, and the TS21's turned out to be about 1.75 mm. If the TS26's don't match, use loose leads.
- **"Nothing echoed back" on the pads means stop.** Something's holding the line down. Take the probe off and measure ([flashing.md](flashing.md#if-it-doesnt-talk)).
- **Never put the meter in series above 3 A.** Turbo draws more than the meter's 10 A range, and the level that gets to 3 A isn't known yet. Start low and ramp up slowly, and stop at 3 A whatever the level.
- **Stay with it in short bursts above level 75.** The thermal constants have never met this head, and the cell's unprotected. A hand on the head is the only thermometer here, so stop when it's uncomfortable. `ramp::CEILING` holds the light at level 120 until stage 7 is done.
- **Don't take the cell under 2.8 V.** The firmware should stop it sooner, but that's one of the things being tested. `first-light` has no protection at all, so don't leave a cell in it.
- **Keep the two backups apart.** `cargo xtask backup` always writes to `backup/`, and nothing in a backup says which chip it came from. Move the breakout's to `backup-breakout/` before stage 1, or `restore` could put seesaw on the torch.

Any firmware problem below is recoverable while the chip still talks: flash and EEPROM are all that ever change, so `cargo xtask restore <port>` puts the stock light back whatever state it's in. A torch that stops talking isn't recoverable by anything in this plan.

## Stage 0: the breakout

Every step of the first flash, rehearsed on a chip that doesn't matter. Wired as in [the spare chip kit](#the-spare-chip-kit), powered by the Friend, with the LEDs off the pins while the meter's on them, since they'd drag the reading. Volts are against GND, and VCC is whatever the meter reads on VIN.

The breakout ships running seesaw, so if the chip's locked, 0.2 will say it didn't enter programming mode. Stop there: the programmer won't unlock a chip.

This proves `xtask` against a real chip using the Friend. It doesn't prove the JLHawaii808 kit, whose probe won't fit the breakout's headers. A failure here might also be the Friend's pulse rather than `xtask`, so a flaky result isn't a verdict on the torch's adapter.

| # | Do | Needs | Expect | Rehearses |
|---|---|---|---|---|
| 0.1 | `cargo xtask ping <port>`, ten times | | `found tinyAVR` and the same string every time (a TS21 says `P:0D:0-3`). A first attempt that only works on the retry would be the pulse holding UPDI in reset | 1.7 |
| 0.2 | `cargo xtask backup <port>`, then rename `backup/` to `backup-breakout/` | | `flash.hex`, `eeprom.hex`, `fuses.hex` | 1.8 |
| 0.3 | Read `backup-breakout/fuses.hex` against `selftest::Fuses::ok`, as 1.9 | | Seesaw might not use the factory fuses. If it fails, the full firmware caps itself here for a good reason, so 0.11 can't work. Don't write the fuses | 1.9 |
| 0.4 | Flash `first-light` as in [flashing.md](flashing.md#the-first-flash-is-first-light) | First light | Verify passes. `cargo xtask hex` on that commit says 628 bytes | 2.2 |
| 0.5 | LEDs on. 1C on, 1C off, a few dozen times | First light | Both LEDs follow the clicks | 3.2 |
| 0.6 | LEDs off, light on. Meter on frequency on SCL, then DC volts | First light | 19.6 kHz, which is 10 MHz / (2 × 255). A wrong clock is unmistakable. DC volts should be about 3.9% of VCC (duty 10 of 255), about 130 mV at 3.3 V | The 10 MHz clock and dual-slope PWM on silicon |
| 0.7 | DC volts on PB2's pin, off, then on, then through a power cycle by the Friend | First light | 0 V off, VCC on, and never high after a power-up without a click | PB2's order, as far as a meter can |
| 0.8 | LED on SCL. 1C on, then `cargo xtask peek <port>` (from main, which has it). Then 1C on again and `cargo xtask ping <port>`. Repeat a few times | First light | `peek` reads before UPDI's turned off, so `VPORTB.OUT` with PB2 (0x04) set shows the chip came through the port opening, the pulse and UPDI's enable without a reset. The datasheet doesn't say either way. Then the light going out as each command ends is the System Reset §33.3.2.2.2 says UPDIDIS issues, so that's the expected result, and the light staying on means that reset doesn't happen here. PB2 clear in `peek` means something earlier reset it, which on this adapter could be the pulse. That only decides how far to trust stage 0's results, since the Friend isn't going near the torch. Write it all down | The pulse, and whether `ping` and `peek` reset a running chip |
| 0.9 | `cargo xtask flash <port>`, then `cargo xtask restore <port>` with `backup-breakout/` put back as `backup/`, then `flash` again | Main | All verify. Seesaw comes back in the middle, which proves `restore` before the torch needs it | 4.1, and `restore`, which nothing else tests |
| 0.10 | Clicks, holds and a dozen power cycles | Main | Same feel as the docs/ui.md rows. Dark from a power-up, no reset loop. A loop shows as the PB2 LED never staying lit | 4.3 |
| 0.11 | 2C for the top of the ramp, then DC volts on SCL | Main | About 51% of VCC (level 120, 16,637 of 32,640). About 3.6% would mean it's capped at `limits::FAULT_CEILING`, level 50, so go through the suspects in 6.1 here, where it costs nothing | 6.1 |
| 0.12 | The same build, left off for an hour, then the same check | Main | Still 51%. Capped means the watchdog bit while it rested, even with `main` idling the tick after each feed | 6.2 |
| 0.13 | T3: a build that stops feeding the watchdog when you hold the switch for ten seconds | T3 | The light goes out within about 100 ms, comes back off, and won't go over level 50 until power's cycled | Nothing on the torch. T3 never goes there |
| 0.14 | Develop T2 here | T2 | Blinks a number you can read | 7.2 |
| 0.15 | T4: power-down straight after a feed. Pick a level away from 52, click off, leave it an hour, then 1C, then 2C and DC volts on SCL | T4 | Comes back at the level you left, and 51% at the top. Back at 52 means the chip reset, and 3.6% means a bite | 6.4 |

Stop at anything unexplained. What goes wrong here would've gone wrong on the torch, except here `flash` puts it right.

Unplug the Friend and put it away before going on.

## Stage 1: the pads, and stock

The last chance to measure Anduril, and the first run of the JLHawaii808 kit on any TS26. [hardware.md](hardware.md#the-flashing-pads) knows there are three pads and nothing else about them, so this stage finds out.

| # | Do | Expect | Closes |
|---|---|---|---|
| 1.1 | Cell voltage on the Fluke, out of the light | 3.6 to 4.2 V. Write it down | |
| 1.2 | On stock, tail current at the lowest level and a few levels up, cap off, meter from cell negative to the tube. Stop at 3 A. Note how the bottom looks and whether it whines | Write them down, as the comparison for stage 5. Any flicker at the bottom is the known low-end flicker, and now you know this light's version of it | |
| 1.3 | Cell out. Find the pads, and photograph them. The driver's glued in, so don't force anything | Three pads. Note any labels | "Find the flashing pads", the open question on where they are |
| 1.4 | Measure the pitch, with calipers or against something of known size | Write it down. If it isn't what the kit's probe is made for, use loose leads. Don't assume 1.27 mm | The open question on pitch |
| 1.5 | Continuity from each pad to the battery negative contact, then ohms from each pad to the positive contact | Exactly one pad is ground. Write down the rest. If no pad is ground, stop | "Check their order with a multimeter" |
| 1.6 | Ohms between each pair of pads, both ways round | Write them down as the baseline. It might drift as something charges from the meter. About 2.5 Ω between all three is what the dead TS21 read, so a reading like that on a healthy torch would be news, and it means stop | |
| 1.7 | Friend away. `cargo xtask ping <port>`, first with the probe touching nothing, then on the pads | In the air, "no reply from the chip", which proves the port and the adapter's echo with no chip at risk. "Nothing echoed back" in the air means the adapter's the problem. On the pads, `found tinyAVR` and something like `P:0D:0-3`. "Nothing echoed back" on the pads means something's pulling the line down, so stop and measure | First half of "ping, back up and flash" |
| 1.8 | `cargo xtask backup <port>`, then copy that directory somewhere safe. It's the only copy of this torch's calibration | `flash.hex`, `eeprom.hex`, `fuses.hex` | Second half |
| 1.9 | Read `backup/fuses.hex` against `selftest::Fuses::ok`. The bytes run `WDTCFG`, `BODCFG`, `OSCCFG`, reserved, `TCD0CFG`, `SYSCFG0`, `SYSCFG1`, `APPEND`, `BOOTEND`, reserved, `LOCKBIT` | `WDTCFG` 0, `OSCCFG.FREQSEL` 20 MHz with `OSCLOCK` clear, `SYSCFG0.RSTPINCFG` UPDI, `APPEND` and `BOOTEND` 0. The TS21's were `00 00 02 FF 00 F6 07 00 00 FF C5` | That the fuse check passes on a real TS26 |
| 1.10 | Read `BODCFG` and `SYSCFG1.SUT` from the same file | The TS21's were `BODCFG` 0x00 and `SUT` 64 ms. This board might differ, and the firmware's hang check wants to know if the BOD's on (see 6.3) | Whether the BOD's on here |

If 1.9 fails, stop. `flash` would still go ahead, since it only checks the signature, and the full firmware would quietly cap itself at level 50. That's worth understanding first.

## Stage 2: first flash

`first-light` only. The full firmware waits for stage 4.

| # | Do | Expect | Closes |
|---|---|---|---|
| 2.1 | Look at the adapter in your hand. Cell out. Ohms between the pads again | The plain kit, and the same numbers as 1.6 | |
| 2.2 | `git switch --detach 9d3b7dc`, `cargo xtask hex`, `cargo xtask flash <port>`, `git switch -` | 628 bytes, and verify passes. A verify only shows the bytes arrived | "Ping, back up and flash", the batched UPDI frames, `PAGEWRITE` |
| 2.3 | If verify fails | Run it once more. If it fails again, suspect `PAGEWRITE`, then the batched frames in `write_page`, and stop there. Only the fake chip has seen either | |
| 2.4 | `cargo xtask ping <port>`, then probe off | Same string as 1.7. A chip that talked before the flash and doesn't after it is the TS21 again, so stop and measure | |

## Stage 3: first-light on the light

The proof that this board does what the repo thinks. In a dark room.

| # | Do | Expect | Closes |
|---|---|---|---|
| 3.1 | Put the cell in and do up the head, watching the emitters. Repeat ten times | No flash at all. PB2 floats through reset until `hw::init` drives it, so a flash here means nothing's holding it low: stop, and look at 3.7 | The open question on what holds PB2 low, by eye |
| 3.2 | 1C on, 1C off, a few dozen times | Dim, steady, and dark again every time. No flash at turn-on, no blip at turn-off. Well clear of Anduril's floor of 20, at 1,280 in its units | "Does it come on cleanly at duty 10, with no flash at turn-on?" |
| 3.3 | Tail current, on | Write it down. It's the first number for what duty does on this driver | Part of "measure what PWM duty does" |
| 3.4 | Tail current, off | A few mA: `first-light` doesn't sleep, and the chip alone is about 3 mA. Much more and something's on that shouldn't be. Anything over the chip's 3 mA is the rest of the driver, which nobody's measured, so write it down | |
| 3.5 | Meter on frequency across the current path, on | 19.6 kHz if it triggers. No reading isn't a failure, since the ripple may be too small | The 10 MHz clock, on the torch |
| 3.6 | Ten minutes on, in sight | Doesn't move, doesn't go out | |
| 3.7 | Cell out, head off: ohms from PB2 to ground, if you can reach the pin | Tens of kΩ or less means a pull-down, which settles the open question. Open means there isn't one, and 3.1's result is all that stands between a reset and a floating enable | "What holds PB2 low" |
| 3.8 | Cell out. Ohms between the pads, then `cargo xtask ping <port>` | Same as 1.6, and the chip still talks | |

**Stage 3 is the gate.** All of it passes, or the full firmware doesn't go on. If the light won't come on, `restore` puts Anduril back, and a stock light that works means the fault's in `first-light` and how it reads this board, which is a far smaller place to look than the full firmware.

Write the result in the [table](#results) before going on.

## Stage 4: the full firmware

| # | Do | Expect | Closes |
|---|---|---|---|
| 4.1 | Back on main, `cargo xtask flash <port>`, then `ping`, then probe off and leave it a few seconds with no cell, so VCC really collapses | Verify passes, and it still talks. A flash doesn't power-cycle the chip, and what Anduril left in the reset flags could otherwise count as a hang | |
| 4.2 | If the light's dark from here on | `cargo xtask peek <port>` first, since only the first run shows how far the firmware got (see 0.8). Then go back to the [steps in flashing.md](flashing.md#flash-this-firmware) and flash `first-light` again. If `first-light` still works, it's the firmware, and the board's fine | |
| 4.3 | Cell in, head on, ten times, watching the emitters | No flash, and it's off until clicked. A reset loop would be dark too, so go on to 4.4 | |
| 4.4 | 1C on, 1C off, a few dozen times, then holds | Same feel as the docs/ui.md rows. The 64 Hz tick has the delta-sigma interrupt running under it now, so sluggish clicks mean the interrupt's eating the loop | |
| 4.5 | Tail current, off, on the 4 A range | Reads 0.000 to 0.003 once it's asleep. Anything steadily over about 5 mA isn't a sleeping chip, or it's a reset loop | The rough version of the off current |
| 4.6 | Aux, off, in lockout (4C) and with the light on | Dim steady blue off, dim steady red in lockout, out while the main LED's lit. hardware.md has PC1 blue, PC2 green, PC3 red from Anduril, so the wrong colour means that's wrong here | "Likely" on the aux pins |
| 4.7 | An hour on at a low level, in sight | Stays on and doesn't move. It going out on its own is the watchdog biting | "It never trips in normal use", in part |
| 4.8 | Leave it off overnight, then 1C | Comes on at once. 6.2 is the real test | |

## Stage 5: the ramp

Short bursts above level 75, and the meter comes out at 3 A.

| # | Do | Expect | Closes |
|---|---|---|---|
| 5.1 | 1H from off, for the lowest level, in the dark | Level 13 lights, steadily. If it doesn't, `ui::FLOOR` doesn't suit this light | That the floor holds on this one |
| 5.2 | The bottom few levels from 13 up, waving the light | Write down any flicker, against reviewers' 1/150 to 5/150 and 1.2. Level 13 was picked to clear that flicker, from a figure on Anduril's curve that nobody's checked on this one | "Is the low-end flicker the driver, the delta-sigma, or both" |
| 5.3 | Ramp slowly end to end | Smooth, no steps that jump, no blip, each direction. The curve's my own cubic, so an ugly spot is a finding, not a surprise | |
| 5.4 | T1 at levels 25, 50, 75, 100: tail current, and stop at 3 A. Those are duties of about 0.5%, 3.6%, 12% and 29% | Plot current against duty. That's the first picture of what turns PB0 into current, filtered reference or otherwise | "Measure what PWM duty does to output" |
| 5.5 | Ear to the head at low and middle levels | Any whine, compared with 1.2 | |
| 5.6 | Top of the ramp, a few seconds: cell volts across the cell | Write down the sag. Level 120 is a duty of 51%, and nobody knows what current that is | |
| 5.7 | Same run, in the dark, off and on a few times | No blip on either edge. Anduril's note that this driver can't pulse under 2 ms suggests a slow start. A visible lag is fine and a flash isn't | |

## Stage 6: faults and the watchdog

There's no proof at power-on here, on purpose, so this is where anyone first sees it work. A fault only shows as the light stopping at level 50, so every check here is about getting above it.

| # | Do | Needs | Expect | Closes |
|---|---|---|---|---|
| 6.1 | Top of the ramp, right after a flash | Main | Clearly brighter than level 50. If not, it's capped. Suspect, in order, a bite (a cell swap clears it), then `xtask/src/crc.rs`, then the fuses ([flashing.md](flashing.md#if-its-capped-at-level-50)) | The CRC convention, the fuse check |
| 6.2 | Leave it off for an hour, then the top of the ramp | Main | Not capped. `main` idles the tick after each feed, so a `wdr` gets to sync with the clock running. Capped means the watchdog bit it anyway while it rested | That a resting torch doesn't bite |
| 6.3 | Note the cap goes when the cell comes out and back | Main | It does. The hang flag lasts until the next reset. A brown-out reset would clear it, if 1.10 says the BOD's on | |
| 6.4 | After 6.2 and 0.15 pass, T4 on the torch: power-down straight after a feed. Pick a level away from 52, click off, then leave it overnight, watching the aux in the dark for a while first. Then 1C, then the top of the ramp. Tail current off too, on the 4 A range | T4 | Back at the level you left, not capped, steady aux, and 0.000 to 0.003 A. Back at 52 means the chip reset. Capped means a bite. A flickering aux or steady mA means it's resetting over and over, so a `wdr` doesn't survive an immediate power-down: flash main again and keep the idle. If all of it passes, the idle can go, which saves about 0.37 mA: drop `&& !fed` from `src/main.rs` and `REST_IDLE_TICKS` from `xtask/src/sim.rs` | "Test power-down straight after a feed" |

A real bite only gets seen on purpose on the breakout (0.13). On the torch, the check is that 4.7 and 6.2 never see one, which is as far as this kit can say the window suits a real chip.

## Stage 7: voltage and heat

| # | Do | Needs | Expect | Closes |
|---|---|---|---|---|
| 7.1 | Run a cell down, with the meter across it between bursts, then a middling level, checking the cell | Main | By `src/battery.rs`, under 3.00 V for five seconds it steps down, and under 2.90 V for a second it fades down the ceiling (120, 90, 60, 30, 1) and turns off in about 7 s. It steps back up at 3.30 V. All loaded voltages, so rest voltage will read higher, and all unmeasured | The roadmap's low-voltage item, on a real light |
| 7.2 | Compare a blinked-out voltage with the Fluke at three or four charge levels, off and on | T2 | Within 0.1 V. This is what tests the 1094 calibration, and the 4.03 ratio behind it, on this light | "Check against a multimeter", the open question on 1094 |
| 7.3 | Near the cut-off, off for a minute, then on | Main | It starts. The MP3432 needs 2.7 V on VIN to start and 0.8 V to keep going, and the 2.90 V cut-off only leaves 0.2 V over that, so a nearly flat cell might run and then not restart. A sag at turn-on is the thing to watch for | The 2.7 V start-up against the cut-off |
| 7.4 | Blink out the temperature at a stable room, three or four readings | T2 | Near the room, ±3 °C by the datasheet. There's no reference to hold it to, so this only finds gross errors | Part of the thermal item |
| 7.5 | Blink out several temperature readings, light off, then at a low, a middle and a high level | T2 | How far they scatter, one against the next | The sensor's noise with the light on |
| 7.6 | Top of the ramp until it steps down, with a thermometer on the head | A thermometer | It steps down before the head's painful, near 50 °C at the sensor, and doesn't go under level 50. 1Lumen's head peaked at 52 °C. Only after this does `ramp::CEILING` get raised, a step at a time | "Thermal regulation", the `ramp::CEILING` decision |
| 7.7 | Charge it over USB with the cell in, watching for the light or a lost level | Main | Nothing odd. A lost level means the charger pulled VCC down far enough to reset the chip | "Level survives charging" |

## Test builds

None exist yet, and they should stay on a throwaway branch: they aren't the firmware. Build and debug each on the breakout first.

**T1, another level.** A build that comes on at a set level. The docs/ui.md examples expect the real one, so `cargo xtask hex` won't write it as it stands. It needs a small `xtask` change that skips the examples, keeps everything else and writes the CRC as usual, since a hex made some other way misses the CRC and caps the light. `cargo xtask flash <port> <file>` already takes any hex.

**T2, a number out.** A build that blinks a value out on the main LED, tens on long blinks and units on short, at a low level. One build per value is fine. Blink codes stay out of the real firmware.

**T3, a hang on demand.** A build that stops feeding the watchdog when the switch has been held for ten seconds, so a real bite can be seen once. **Breakout only.** It's the only deliberate reset anywhere in this plan, and it isn't going near the torch: the real firmware doesn't do this at start-up, by choice, because of what happened to the TS21.

**T4, power-down after a feed.** `watchdog::Feeder::power_down` in `src/watchdog.rs` returning `resting` alone, dropping the `&& !self.fed`. The simulator checks the firmware against that same function, so it follows without a change. It's the one test build that goes on the torch, after 0.15 passes, and only to find out if the idle in the real firmware can go. It has the real firmware's protection and nothing else changed.

## What this kit can't do

| Check | Why | What would unlock it |
|---|---|---|
| Off current to the µA | No mA or µA range | A 1 kΩ resistor in series with the cell: 1 mV per µA, on the 600 mV range. The firmware waking each tick will wobble it, and the 3 readings a second will average that |
| What the supply pad does with the cell in or out, and what VCC really is | The pads are under the head, and the head has to be on for the light to run | A wire from the body to the head, so the meter can reach the pads with the head off |
| PB2 and PB0 through a real reset, on the torch | Too quick for the meter | A scope. 3.1 by eye is the stand-in |
| The turn-on and turn-off order, against the MP3432's 180 µs | Needs a scope on EN and on the PWM | A scope |
| The real divider ratio on PA1 | The pin's on the board, and the sense point might not be reachable | Probing PA1 with a fine tip, cell in and light off, against the cell. Only with a tip that can't slip |
| Which chip the boost converter is | Glued in, and possibly under potting | Reading the marking through a loupe if the driver can be seen at all. Don't dig |
| Sensor against a real temperature, thermal tuning | No thermometer | Any thermometer on the head |
| 4.2 V to 2.5 V sweep | No bench supply | A bench supply. 7.1 covers the part that matters |

Until those happen, docs/hardware.md's labels stay as they are. Move a fact from likely to confirmed only with a result from here beside it, and say so if the result came from the breakout rather than a TS26.

## Results

Fill this in as you go, with the commit you flashed.

| # | Date | Commit | Result |
|---|---|---|---|
| | | | |
