# Bench plan

Every hardware check the roadmap is waiting on, in the order to do them, for a bench with the torch, a Fluke 107, [JLHawaii808's 4.5 V UPDI kit](https://jlhawaii808.com/products/updi-programmer-driver-reflashling-kit) for the torch, and the [spare chip kit](#the-spare-chip-kit) (an Adafruit ATtiny1616 breakout, an Adafruit High Voltage UPDI Friend, a breadboard, jumpers, tactile switches, blue LEDs, and a 27 kΩ and a 10 kΩ resistor). Nothing here has run on a TS26 yet, so treat each expected result as a prediction. [flashing.md](flashing.md) has how to use the programmer.

The order is the point of it. **The first flash on the torch is `first-light` (commit `9d3b7dc`), and the full firmware only goes on after stage 3 has passed.** An earlier torch went dark straight after a verified flash of a much bigger start-up, and nobody could say whether that was firmware or hardware. A 628-byte image that clicks on and off keeps those two apart.

Each check says what it needs.

- **First light** is `first-light`, the firmware as of commit `9d3b7dc`, built in its own worktree and flashed with main's programmer.
- **Main** is the full firmware on main.
- **T1** to **T5** are each a [test build](#test-builds).

The Status column says where each row stands. It's blank until the row's been done, then Pass, Fail or Skipped. A Fail isn't always a stop, so read the row's own words for what follows, and put the detail in [Results](#results).

The firmware passing the simulator doesn't change any of this. It's a mistake-finder written from the same reading of the datasheet as the firmware, and it doesn't model a chip reset or anything analogue, so none of its passes count as a result here.

Checks this kit can't do at all are at [the end](#what-this-kit-cant-do), with what would unlock them.

## What the kit can and can't see

Here's the [Fluke 107](https://www.fluke.com/en-us/product/electrical-testing/digital-multimeters/fluke-107), from Fluke's spec table.

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

The breakout is the same chip as the torch with nothing expensive attached, so everything risky happens there first. Its pins are also the one place the meter gets a clean logic signal. On the torch the PWM is buried in the driver, and here it's a header pin.

The header names are the board's silkscreen. They're from Adafruit's Eagle schematic and board files at commit `8d8df66` of [the breakout's repo](https://github.com/adafruit/Adafruit-ATtinyx16-Breakouts-PCB), with the silkscreen text read out of the `.brd`, and they agree with megaTinyCore's `variants/txy6/pins_arduino.h`. Confirmed.

| Torch | Chip pin | Breakout header | What's different |
|---|---|---|---|
| PWM | PB0 | SCL | 10 kΩ pull-up to VCC, and both STEMMA QT sockets |
| Side switch | PA5 | 1 | Nothing else on it |
| UPDI pad | PA0 | UPDI | 10 kΩ pull-up to VCC |
| Battery sense | PA1 | 14 | No cell behind it, so the wiring below gives it a divider from Vin. The full firmware reads the cell here, and left floating it sees whatever the pin drifts to. By `src/battery.rs` that steps it down or turns it off as a low cell, or caps it at level 50 and turns it off after five minutes as a cell it can't read, which rows 0.10 to 0.15 would then blame on something else |
| Supply pad | VCC | Vin | VCC is Vin, through a cuttable trace on solder jumper SJ1, so the chip runs at whatever goes on Vin. The AP2127K-3.3 regulator only feeds the 3V pin, which is an output, so don't feed it |
| Boost enable | PB2 | 7 | The schematic's net is TXD, since PB2 is USART0's default TX pin, but the silkscreen says 7 |
| Aux | PC1, PC2, PC3 | 11, 12, 13 | |
| Ground | GND | G | |

The board has two LEDs of its own, a red one on PC0 (active low) and a green power LED on VCC. Neither is on a pin the torch uses.

Here's the wiring, with the Friend's slide switch on 3 V.

- Friend's UPDI, power and GND to the breakout's UPDI, Vin and G.
- A tactile switch from pin 1 to G. Mind which legs are joined, since the 12 mm ones have four.
- A blue LED from SCL to G and a second from pin 7 to G, cathodes to G.
- 27 kΩ from Vin to pin 14 and 10 kΩ from pin 14 to G, to stand in for the cell. At 3.3 V that's about 0.89 V on the pin, which the full firmware should read as about 3.6 V (arithmetic at the 4.03 ratio, untried). Check it with the meter before 0.9. Anything from 0.80 to 1.05 V will do, which reads as 3.2 to 4.2 V, clear of the 3.00 V step-down and of the 4.40 V top of what it takes for a real cell. Other pairs between about 2.2 and 3 to 1 land in that range too. `first-light` never reads the pin, so it can stay wired at the kit's 4.5 V.

At 3.3 V a blue LED's forward voltage does most of the current limiting, so it's fine straight on a pin for this. At 5 V only the pin's own drive limits it, which could pass the 40 mA absolute maximum (DS40002204A table 36-1), so **keep the switch on 3 V** or add 1 kΩ in series. The same goes for the JLHawaii808 kit's 4.5 V in rows 0.16 on, since VCC is whatever's on Vin. Keep LEDs off the pins, or put 1 kΩ in series. The header strips come with the board and need soldering.

The breakout can't tell you anything about the driver. There's no MP3432, no regulator and no emitters. PB0's pull-up means SCL goes high through a reset where the torch's pin floats, so a glow on that LED at power-on is expected here and says nothing about the torch. And it's a different chip from the torch's, possibly another silicon revision, so a pass here makes a torch pass likely without closing a roadmap item.

## The High Voltage UPDI Friend's 12 V pulse

Read this before plugging the Friend into anything. It's a serial UPDI adapter (CH340E, 1 kΩ from TX to RX, and it powers the target at 3.3 V or USB's 5 V), and it **never goes on the torch**, not for flashing and not for recovery.

Its [schematic](https://github.com/adafruit/Adafruit-High-Voltage-UPDI-Friend-PCB) has RTS# going through a 10 nF capacitor to the control pin of an analogue switch, which a 10 kΩ resistor holds high. High connects the UPDI line to the serial port. Low connects it to the 12 V rail, which is always up. So a falling edge on RTS# puts 12 V on UPDI for roughly 100 µs (a reading of the RC, not a spec), and no switch or jumper stops it. From the feedback resistors the rail should be about 12.4 V, if the MT3608's reference is the usual 0.6 V.

Opening a serial port normally asserts RTS, which is that falling edge, and `xtask/src/serial.rs` can't prevent it, because the kernel does it during `open`, before `stty` runs. So assume **every `cargo xtask` command fires one pulse as it starts.** Likely, not confirmed. The CH340 driver on macOS might behave differently, and the meter's too slow to see it.

Here's what the datasheet says about that (DS40002204A).

- The pin takes it, on paper. The absolute maximum on RESET is 13 V (table 36-1), and detection is 11.5 to 12.5 V (table 36-11). But the same page warns that overshoot past 13 V can latch the pin's ESD protection on, and a 12 V driver can then push enough current through it to do permanent damage. Nothing here has measured any board's overshoot.
- It only describes the pulse as a way into UPDI from another pin function (§33.3.2.1.3). What it does to a chip already set to UPDI isn't stated.
- It's only rated for the RESET pin. On any other pin the limit is VDD + 0.5 V. **Never put the Friend's UPDI line on another pin**, on the breakout or anywhere else.

An earlier torch was lost on 2026-11-06 after the Friend was hand-held on its pads, and it then measured about 2.5 Ω between all three. The likely cause is 12 V or a slipped probe, and that isn't proven. It doesn't need to be. There's a kit that does the job without 12 V, and on the TS26 nobody even knows what else is on the pads.

## Rules

- **Only the plain adapter on the torch.** The Friend never touches it, recovery included. Both adapters show up as CH340 `1a86:7523`, so nothing on screen tells you which you've got. Unplug the Friend and put it away before the head comes off, and look at the adapter before every command.
- **Nothing here writes fuses.** If a fuse looks wrong, stop and decide with the datasheet open. A bad `SYSCFG0` would need 12 V to undo, and that means the torch is gone.
- **Cell out before anything touches the pads.** The programmer powers the chip through the supply pad, and nobody knows yet what that does to the regulator. Check the ground pad is continuous with the battery negative first, every time you've had it apart.
- **Name all three pads before anything touches them.** Continuity finds ground, and 1.5 tells the supply pad from the UPDI pad by how each reads against it. No public source gives the TS26's pad order, and the board's own `R - +` silkscreen (1.3) is a label nobody has checked with a meter. A wrong guess puts the adapter's 4.5 V supply on PA0 and its data line on the supply rail, with the chip unpowered. That's inside the 13 V absolute maximum the datasheet gives the RESET pin, which PA0 is (DS40002204A table 36-1), and well outside the VDD + 0.2 V it gives as that pin's input level (table 36-11). Nothing describes a chip driven that way round, and nobody knows what else is on either pad. The kit's own three pins get named first, in 0.16. If either naming is in doubt, stop.
- **Don't hold the probe by hand if you can avoid it.** Clamp it, or use something else that can't slip. A slip across pads this close together shorts them.
- **Measure before trusting a probe.** The JLHawaii808 probe is made for 1.27 mm pads, and an earlier torch's turned out to be about 1.75 mm. This TS26's fit it (1.4). If another board's don't, use loose leads.
- **"Nothing echoed back" on the pads means stop.** Something's holding the line down. Take the probe off and measure ([flashing.md](flashing.md#if-it-doesnt-talk)).
- **Never put the meter in series above 3 A.** Turbo draws more than the meter's 10 A range, and the level that gets to 3 A isn't known yet. Start low and ramp up slowly, and stop at 3 A whatever the level.
- **Stay with it in short bursts above level 75.** The thermal constants have never met this head, and the cell's unprotected. A hand on the head is the only thermometer here, so stop when it's uncomfortable. `ramp::CEILING` holds the light at level 120 until stage 7 is done.
- **Don't take the cell under 2.8 V.** The firmware should stop it sooner, but that's one of the things being tested. `first-light` has no protection at all, so don't leave a cell in it.
- **Keep the two backups apart.** `backup` and `restore` take a directory after the port, and each chip gets its own, `backup/breakout` for the breakout and `backup/torch` for the torch. The path is used as given, so run every command here from the repo root. `backup` saves the chip's serial number beside the images, and `restore` refuses a backup that was read from another chip, so the breakout's seesaw can't go on the torch that way. Until stage 0 that check has only met the fake chip, and it can't tell two backups of the same chip apart, so still read the directory in the command before you run it.

Any firmware problem below is recoverable while the chip still talks. Flash and EEPROM are all that ever change, so `cargo xtask restore <port> backup/torch` puts the stock light back whatever state it's in. A torch that stops talking isn't recoverable by anything in this plan.

## Stage 0, the breakout

Every step of the first flash, rehearsed on a chip that doesn't matter. Wired as in [the spare chip kit](#the-spare-chip-kit), powered by the Friend until 0.16, with the LEDs off the pins while the meter's on them, since they'd drag the reading. Volts are against G, and VCC is whatever the meter reads on Vin.

The breakout ships running seesaw, so if the chip's locked, 0.2 will say it didn't enter programming mode. Stop there, because the programmer won't unlock a chip.

Rows 0.1 to 0.15 prove `xtask` against a real chip using the Friend. A failure there might be the Friend's pulse rather than `xtask`, so a flaky result isn't a verdict on the torch's adapter. Rows 0.16 to 0.20 put the Friend away and run the JLHawaii808 kit against the same chip, so the torch isn't the first chip that adapter ever meets. The kit's probe won't sit on the breakout's headers, so that's on jumper leads, and it proves the adapter and its pin order. It can't prove the probe's fit on the pads, or anything about the torch's board.

| # | Status | Do | Needs | Expect | Rehearses |
|---|---|---|---|---|---|
| 0.1 | Pass | `cargo xtask ping <port>`, ten times | | `found tinyAVR` and the same string every time (another ATtiny1616 board said `P:0D:0-3`). A first attempt that only works on the retry would be the pulse holding UPDI in reset | 1.7 |
| 0.2 | Pass | `cargo xtask backup <port> backup/breakout` | | `flash.hex`, `eeprom.hex`, `fuses.hex` and `serial.hex` in `backup/breakout`, and no warning about the serial number. Write the number down, since it's the first one anyone here has read | 1.8 |
| 0.3 | Fail | Read `backup/breakout/fuses.hex` against `selftest::Fuses::ok`, as 1.9 | | Seesaw might not use the factory fuses. If it fails, the full firmware caps itself here for a good reason, so 0.11 can't work. Don't write the fuses | 1.9 |
| 0.4 | Pass | Build `first-light` in its own worktree and flash it from main, as in [flashing.md](flashing.md#the-first-flash-is-first-light) | First light | `cargo xtask hex` in the worktree says 628 bytes. `flash` warns that the image isn't the full 16,384 bytes with a CRC, which is expected for `first-light`, and verify passes | 2.2 |
| 0.5 | Pass | LEDs on. 1C on, 1C off, a few dozen times | First light | Both LEDs follow the clicks | 3.2 |
| 0.6 | Pass | LEDs off, light on. Meter on frequency on SCL, then DC volts | First light | 19.6 kHz, which is 10 MHz / (2 × 255). A wrong clock is unmistakable. DC volts should be about 3.9% of VCC (duty 10 of 255), about 130 mV at 3.3 V | The 10 MHz clock and dual-slope PWM on silicon |
| 0.7 | Pass | DC volts on pin 7 (PB2), off, then on, then through a power cycle by the Friend | First light | 0 V off, VCC on, and never high after a power-up without a click | PB2's order, as far as a meter can |
| 0.8 | Pass | LED on SCL. 1C on, then `cargo xtask peek <port>` (from main, which has it). Then 1C on again and `cargo xtask ping <port>`. Repeat a few times | First light | `peek` reads before UPDI's turned off, so `VPORTB.OUT` with PB2 (0x04) set shows the chip came through the port opening, the pulse and UPDI's enable without a reset. The datasheet doesn't say either way. Then the light going out as each command ends is the System Reset §33.3.2.2.2 says UPDIDIS issues, so that's the expected result, and the light staying on means that reset doesn't happen here. PB2 clear in `peek` means something earlier reset it, which on this adapter could be the pulse. That only decides how far to trust stage 0's results, since the Friend isn't going near the torch. Write it all down | The pulse, and whether `ping` and `peek` reset a running chip |
| 0.9 | Pass | `cargo xtask flash <port>`, then `cargo xtask restore <port> backup/breakout`, then `flash` again | Main | All verify, and `restore` says the chip's serial number is the one saved in the directory. Seesaw comes back in the middle, which proves `restore` and its serial check before the torch needs them | 4.1, and `restore`, which nothing else tests |
| 0.10 | Pass | Clicks, holds and a dozen power cycles | Main | Same feel as the docs/ui.md rows. Dark from a power-up, no reset loop. A loop shows as the LED on pin 7 never staying lit | 4.3 |
| 0.11 |  | 2C for the top of the ramp, then DC volts on SCL | Main, or T5 on a breakout that fails 0.3 | About 51% of VCC (level 120, 16,637 of 32,640). About 3.6% would mean it's capped at `limits::FAULT_CEILING`, level 50, so check pin 14 still reads 0.80 to 1.05 V, then go through the suspects in 6.1 here, where it costs nothing | 6.1 |
| 0.12 |  | The same build, left off for an hour, then the same check | Main, or T5 on a breakout that fails 0.3 | Still 51%. Capped means the watchdog bit while it rested, even with `main` idling the tick after each feed | 6.2 |
| 0.12a |  | Light off. Move pin 14's lead from the divider to G, then 2C, with the meter on SCL and a clock running. At the end 1C again, then put the divider back | Main | About 3.6% of VCC within a couple of seconds, which is the level 50 cap for a cell it can't read. Out by itself five minutes after it came on. 1C brings it back, still capped. Back on the divider, 2C climbs back to 51% within half a minute | Nothing on the torch, whose PA1 can't be unplugged. It's the only time anyone sees this shut-off on a chip |
| 0.13 |  | T3, a build that stops feeding the watchdog when you hold the switch for ten seconds | T3 | The light goes out within about 100 ms, comes back off, and won't go over level 50 until power's cycled | Nothing on the torch. T3 never goes there |
| 0.14 |  | Develop T2 here | T2 | Blinks a number you can read | 7.2 |
| 0.15 | Pass | T4, power-down straight after a feed. Pick a level away from 52, click off, leave it an hour, then 1C, then 2C and DC volts on SCL | T4 | Comes back at the level you left, and 51% at the top. Back at 52 means the chip reset, and 3.6% means a bite | 6.4 |
| 0.16 | Pass | Unplug the Friend and put it away. Then name the kit's three pins before they touch anything. Unplugged, continuity from each to the USB plug's shell, then plugged in, DC volts from the other two to that one. Then its three lines to the breakout's UPDI, Vin and G, on jumper leads, or with its pogo pins held on the header pins. LEDs off the pins, or 1 kΩ in series | | One pin is continuous with the shell, and that's ground (likely, since a USB shell is normally grounded, and this kit's hasn't been checked). The other two should both read high, the supply at about 4.5 V and the UPDI line because a serial line idles high, so voltage alone doesn't say which is which. Go by the kit's markings, and 0.17 proves them. On the breakout, about 4.5 V on Vin, and the green power LED lit | The probe's pin order, for 1.7 |
| 0.17 | Pass | `cargo xtask ping <port>`, first with the leads off the breakout, then on it | | Off it, "no reply from the chip", which proves the port and the kit's echo. On it, `found tinyAVR` and the same string as 0.1. That settles 0.16's names too. With the supply and UPDI lines swapped it wouldn't answer (reasoning), and here that costs nothing | 1.7, with the adapter that goes on the torch |
| 0.18 | Pass | `cargo xtask backup <port> backup/breakout-kit` | | `flash.hex`, `eeprom.hex` and `fuses.hex` in `backup/breakout-kit`. The flash is whatever build the chip holds by now, so it's only proof that `backup` works through this adapter. Its `fuses.hex` and `serial.hex` should match `backup/breakout`'s byte for byte, since nothing here writes fuses and it's the same chip | 1.8 |
| 0.19 | Pass | Flash `first-light` from main, as 0.4. Then a click, with the meter on pin 7 | First light | The same warning, and verify passes. 0 V off and about VCC on, so it runs at 4.5 V too | 2.2, with the adapter and the command the torch gets |
| 0.20 | Pass | `cargo xtask restore <port> backup/breakout`, then `ping` | | Verifies, and it still talks. Seesaw's back, and `restore` has run through this adapter | `restore`, before the torch needs it |

Stop at anything unexplained. What goes wrong here would've gone wrong on the torch, except here `flash` puts it right.

The Friend's been away since 0.16. Check it still is before going on.

## Stage 1, the pads, and stock

The last chance to measure the stock firmware, and the first run of the JLHawaii808 kit on any TS26. [hardware.md](hardware.md#the-flashing-pads) knows where the three pads are and what the silkscreen calls them, from 1.3, and that their pitch fits the kit's probe, from 1.4, and nothing about what each connects to, so the rest of this stage finds out.

| # | Status | Do | Expect | Closes |
|---|---|---|---|---|
| 1.1 | Pass | Cell voltage on the Fluke, out of the light | 3.6 to 4.2 V. Write it down | |
| 1.2 | Skipped | On stock, tail current at the lowest level and a few levels up, cap off, meter from cell negative to the tube. Stop at 3 A. Note how the bottom looks and whether it whines | Write them down, as the comparison for stage 5. Any flicker at the bottom is the known low-end flicker, and now you know this light's version of it | |
| 1.3 | Pass | Cell out. Find the pads, and photograph them. The driver's glued in, so don't force anything | Three pads. Note any labels | "Find the flashing pads", the open question on where they are |
| 1.4 | Pass | Measure the pitch, with calipers or against something of known size | Write it down. If it isn't what the kit's probe is made for, use loose leads. Don't assume 1.27 mm | The open question on pitch |
| 1.5 | Pass | Continuity from each pad to the battery negative contact. Then ohms from each of the other two pads to the ground pad, watching how the reading moves over a few seconds. Then diode mode from each pad to the battery positive contact, both ways round | Exactly one pad is ground. If none is, stop. The other two get their names here, from a prediction that's reasoning from the datasheet and unmeasured. The supply pad has the board's decoupling capacitors on it, so it should start low, climb as the meter charges them, and settle. The UPDI pad should have nothing on it but the chip's pin, and the pin's pull-up is inside the chip and only works while the chip's powered (DS40002204A §33.3.2.1.2), so it should read open or very high from the first moment, and never low. The pull-up itself is 20 to 60 kΩ (table 36-11), and nobody knows if it conducts with the chip unpowered. If the two pads don't differ clearly enough to name them with confidence, stop. Against battery positive, a forward drop from the supply pad (red lead on the pad), or any low reading, means the programmer will power the cell rail as well as the chip. That isn't a stop on its own, but it's what 1.7 watches for. Open both ways makes it less likely and doesn't rule it out, since the meter only tries the path with nothing powered (reasoning). Write all of it down | "Check their order with a multimeter" |
| 1.6 | Pass | Ohms between each pair of pads, both ways round. Then diode mode between the UPDI pad and the supply pad, both ways round | Write them down as the baseline. Supply to ground is the pair that should drift, as in 1.5, and UPDI to supply might read the pull-up's 20 to 60 kΩ, or open. In diode mode an ordinary pin would show its protection diode to the supply, one drop one way round and open the other. PA0 is the one pin rated to 13 V whatever VDD is (table 36-1), so that diode isn't expected on it, and other parts on the board could mask it either way (reasoning, unmeasured). So that reading's one to write down, and 1.5 is what names the pads. If anything here disagrees with 1.5's names, stop. A few ohms between all three pads is a short, and that means stop too | |
| 1.7 |  | Friend away. `cargo xtask ping <port>`, first with the probe touching nothing, then on the pads. Make that first contact in a dark room, watching the emitters, with a finger on the adapter | In the air, "no reply from the chip", which proves the port and the adapter's echo with no chip at risk. "Nothing echoed back" in the air means the adapter's the problem. On the pads, `found tinyAVR` and something like `P:0D:0-3`. "Nothing echoed back" on the pads means something's pulling the line down, so stop and measure. Dark and cool the whole time. Any glow from the emitters, or warmth in the adapter, is a stop, so lift the probe and read [what the adapter powers](#what-the-adapter-powers) again | First half of "ping, back up and flash" |
| 1.8 |  | The same watch as 1.7, since this is the first time the chip sits in programming mode. `cargo xtask backup <port> backup/torch`, then copy that directory somewhere safe. It's the only copy of this torch's calibration | `flash.hex`, `eeprom.hex`, `fuses.hex` and `serial.hex` in `backup/torch`, and dark and cool throughout. The serial number should differ from the breakout's. If `backup` warns that it can't tell one chip from another, stop before flashing anything, since `restore` would refuse this backup. If it's the same as the breakout's, the check proves nothing here, so go by the directory name alone | Second half |
| 1.9 |  | Read `backup/torch/fuses.hex` against `selftest::Fuses::ok`. The bytes run `WDTCFG`, `BODCFG`, `OSCCFG`, reserved, `TCD0CFG`, `SYSCFG0`, `SYSCFG1`, `APPEND`, `BOOTEND`, reserved, `LOCKBIT` | `WDTCFG` 0, `OSCCFG.FREQSEL` 20 MHz with `OSCLOCK` clear, `SYSCFG0.RSTPINCFG` UPDI, `APPEND` and `BOOTEND` 0. Another ATtiny1616 board's were `00 00 02 FF 00 F6 07 00 00 FF C5` | That the fuse check passes on a real TS26 |
| 1.10 |  | Read `BODCFG` and `SYSCFG1.SUT` from the same file | Another ATtiny1616 board's were `BODCFG` 0x00 and `SUT` 64 ms. This board might differ, and the firmware's hang check wants to know if the BOD's on (see 6.3) | Whether the BOD's on here |

If 1.9 fails, stop. `flash` would still go ahead, since it only checks the signature, and the full firmware would quietly cap itself at level 50. That's worth understanding first.

### What the adapter powers

With the cell out, the adapter feeds the regulator's output through the supply pad. Whether that reaches the cell rail, where the MP3432's input is, is unknown, and the converter only needs 2.7 V there to start. 1.5's reading against battery positive is the only evidence there is before contact.

It matters because the chip doesn't hold the boost enable low for the whole time the probe's on. After a reset every output is tri-stated (DS40002204A §16.3.1), so PB2 floats until firmware drives it. In programming mode, which is all of `backup`, `flash` and `restore`, the chip's been reset and the CPU isn't running (§33.3.8.2), so PB2 floats for as long as the command takes. That's a reading of the two sections together, and [hardware.md](hardware.md#the-driver) has why a pull-down on the board is likely and unseen. `ping` and `peek` don't enter programming mode, so the firmware runs and drives PB2 itself. But the stock firmware turns the main output on for one blink every time it starts ([hardware.md](hardware.md#the-flashing-pads)), and it starts when the probe lands and again when `ping` ends in a reset.

So if the cell rail is fed, the emitters could light with the adapter as their only supply. No report of that has turned up on a boost driver, nor any report of anyone flashing a TS26 at all ([hardware.md](hardware.md#the-flashing-pads)), so the first contact is the test. Any glow or warmth in 1.7 or 1.8 is a stop. A single blink as the probe lands would be the stock firmware's own, and it's still a stop, because it means the adapter is running the converter.

## Stage 2, first flash

`first-light` only. The full firmware waits for stage 4.

| # | Status | Do | Expect | Closes |
|---|---|---|---|---|
| 2.1 |  | Look at the adapter in your hand. Cell out. Ohms between the pads again | The plain kit, and the same numbers as 1.6 | |
| 2.2 |  | `cargo xtask hex` in the `first-light` worktree from 0.4, then from main `cargo xtask flash <port> ../ts26-first-light/target/wurkkos-ts26-0721.hex` ([flashing.md](flashing.md#the-first-flash-is-first-light)) | 628 bytes, the same warning as 0.4 about an image short of 16,384 bytes, and verify passes. A verify only shows the bytes arrived | "Ping, back up and flash", the batched UPDI frames, `PAGEWRITE` |
| 2.3 |  | If verify fails | Run it once more. If it fails again, stop there. `PAGEWRITE` and the batched frames in `write_page` have both written a real chip by now, the breakout, through the Friend in 0.4 and 0.9 and through this adapter in 0.19. If they passed there, suspect the contact on the pads and this board before the programmer (reasoning). A verify that passes still only shows the bytes arrived | |
| 2.4 |  | `cargo xtask ping <port>`, then probe off | Same string as 1.7. A chip that talked before the flash and doesn't after it is how the earlier torch was lost, so stop and measure | |

## Stage 3, first-light on the light

The proof that this board does what the repo thinks. In a dark room.

| # | Status | Do | Expect | Closes |
|---|---|---|---|---|
| 3.1 |  | Put the cell in and do up the head, watching the emitters. Repeat ten times | No flash at all. PB2 floats through reset until `hw::init` drives it, so a flash here means nothing's holding it low. Stop and look at 3.7 | The open question on what holds PB2 low, by eye |
| 3.2 |  | 1C on, 1C off, a few dozen times | Dim, steady, and dark again every time. No flash at turn-on, no blip at turn-off. Well clear of the stock firmware's floor of 20, at 1,280 in its units | "Does it come on cleanly at duty 10, with no flash at turn-on?" |
| 3.3 |  | Tail current, on | Write it down. It's the first number for what duty does on this driver | Part of "measure what PWM duty does" |
| 3.4 |  | Tail current, off | A few mA, since `first-light` doesn't sleep, and the chip alone is about 3 mA. Much more and something's on that shouldn't be. Anything over the chip's 3 mA is the rest of the driver, which nobody's measured, so write it down | |
| 3.5 |  | Meter on frequency across the current path, on | 19.6 kHz if it triggers. No reading isn't a failure, since the ripple may be too small | The 10 MHz clock, on the torch |
| 3.6 |  | Ten minutes on, in sight | Doesn't move, doesn't go out | |
| 3.7 |  | Cell out, head off. Ohms from PB2 to ground, if you can reach the pin | Tens of kΩ or less means a pull-down, which settles the open question. Open means there isn't one, and 3.1's result is all that stands between a reset and a floating enable | "What holds PB2 low" |
| 3.8 |  | Cell out. Ohms between the pads, then `cargo xtask ping <port>` | Same as 1.6, and the chip still talks | |

**Stage 3 is the gate.** All of it passes, or the full firmware doesn't go on. If the light won't come on, `cargo xtask restore <port> backup/torch` puts the stock firmware back, and a stock light that works means the fault's in `first-light` and how it reads this board, which is a far smaller place to look than the full firmware.

Write the result in the [table](#results) before going on.

## Stage 4, the full firmware

| # | Status | Do | Expect | Closes |
|---|---|---|---|---|
| 4.1 |  | From main, `cargo xtask flash <port>` with no file, then `ping`, then probe off and leave it a few seconds with no cell, so VCC really collapses | Verify passes, and it still talks. A flash doesn't power-cycle the chip, and what the stock firmware left in the reset flags could otherwise count as a hang | |
| 4.2 |  | If the light's dark from here on | `cargo xtask peek <port>` first, since only the first run shows how far the firmware got (see 0.8). Then go back to the [steps in flashing.md](flashing.md#flash-this-firmware) and flash `first-light` again. If `first-light` still works, it's the firmware, and the board's fine | |
| 4.3 |  | Cell in, head on, ten times, watching the emitters | No flash, and it's off until clicked. A reset loop would be dark too, so go on to 4.4 | |
| 4.4 |  | 1C on, 1C off, a few dozen times, then holds | Same feel as the docs/ui.md rows. The 64 Hz tick has the delta-sigma interrupt running under it now, so sluggish clicks mean the interrupt's eating the loop | |
| 4.5 |  | Tail current, off, on the 4 A range | Reads 0.000 to 0.003 once it's asleep. Anything steadily over about 5 mA isn't a sleeping chip, or it's a reset loop | The rough version of the off current |
| 4.6 |  | Aux, off, in lockout (4C) and with the light on | Dim steady blue off, dim steady red in lockout, out while the main LED's lit. hardware.md has PC1 blue, PC2 green, PC3 red from the stock firmware, so the wrong colour means that's wrong here | "Likely" on the aux pins |
| 4.7 |  | An hour on at a low level, in sight | Stays on and doesn't move. Going out five minutes after it came on, and coming back with 1C, is a cell voltage it can't read ([flashing.md](flashing.md#if-its-capped-at-level-50)). Going out at any other time is the watchdog biting, or the tick deadline, which ends in the same reset while the oscillator's alive | "It never trips in normal use", in part |
| 4.8 |  | Leave it off overnight, then 1C | Comes on at once. 6.2 is the real test | |

## Stage 5, the ramp

Short bursts above level 75, and the meter comes out at 3 A.

| # | Status | Do | Expect | Closes |
|---|---|---|---|---|
| 5.1 |  | 1H from off, for the lowest level, in the dark | Level 13 lights, steadily. If it doesn't, `ui::FLOOR` doesn't suit this light | That the floor holds on this one |
| 5.2 |  | The bottom few levels from 13 up, waving the light | Write down any flicker, against reviewers' 1/150 to 5/150 and 1.2. Level 13 was picked to clear that flicker, from a figure on the stock firmware's curve that nobody's checked on this one | "Is the low-end flicker the driver, the delta-sigma, or both" |
| 5.3 |  | Ramp slowly end to end | Smooth, no steps that jump, no blip, each direction. The curve is the project's own cubic, so an ugly spot is a finding, not a surprise | |
| 5.4 |  | T1 at levels 25, 50, 75, 100. Tail current, and stop at 3 A. Those are duties of about 0.5%, 3.6%, 12% and 29% | Plot current against duty. That's the first picture of what turns PB0 into current, filtered reference or otherwise | "Measure what PWM duty does to output" |
| 5.5 |  | Ear to the head at low and middle levels | Any whine, compared with 1.2 | |
| 5.6 |  | Top of the ramp, a few seconds. Cell volts across the cell | Write down the sag. Level 120 is a duty of 51%, and nobody knows what current that is | |
| 5.7 |  | Same run, in the dark, off and on a few times | No blip on either edge. The stock firmware's note that this driver can't pulse under 2 ms suggests a slow start. A visible lag is fine and a flash isn't | |

## Stage 6, faults and the watchdog

There's no proof at power-on here, on purpose, so this is where anyone first sees it work. A fault only shows as the light stopping at level 50, so every check here is about getting above it. A cell voltage the firmware can't believe stops it at 50 as well, and that one goes on to turn the light off after five minutes, which no failed check does (0.12a, and [flashing.md](flashing.md#if-its-capped-at-level-50)).

| # | Status | Do | Needs | Expect | Closes |
|---|---|---|---|---|---|
| 6.1 |  | Top of the ramp, right after a flash | Main | Clearly brighter than level 50. If not, it's capped. Suspect, in order, a bite (a cell swap clears it), then `xtask/src/crc.rs`, then the fuses ([flashing.md](flashing.md#if-its-capped-at-level-50)) | The CRC convention, the fuse check |
| 6.2 |  | Leave it off for an hour, then the top of the ramp | Main | Not capped. `main` idles the tick after each feed, so a `wdr` gets to sync with the clock running. Capped means the watchdog bit it anyway while it rested | That a resting torch doesn't bite |
| 6.3 |  | Note the cap goes when the cell comes out and back | Main | It does. The hang flag lasts until the next reset. A brown-out reset would clear it, if 1.10 says the BOD's on | |
| 6.4 |  | After 6.2 and 0.15 pass, T4 on the torch, power-down straight after a feed. Pick a level away from 52, click off, then leave it overnight, watching the aux in the dark for a while first. Then 1C, then the top of the ramp. Tail current off too, on the 4 A range | T4 | Back at the level you left, not capped, steady aux, and 0.000 to 0.003 A. Back at 52 means the chip reset. Capped means a bite. A flickering aux or steady mA means it's resetting over and over, and a `wdr` doesn't survive an immediate power-down. Flash main again and keep the idle. If all of it passes, the idle can go, which saves about 0.37 mA. Drop `&& !fed` from `src/main.rs` and `REST_IDLE_TICKS` from `xtask/src/sim.rs` | "Test power-down straight after a feed" |

A real bite only gets seen on purpose on the breakout (0.13). On the torch, the check is that 4.7 and 6.2 never see one, which is as far as this kit can say the window suits a real chip.

## Stage 7, voltage and heat

| # | Status | Do | Needs | Expect | Closes |
|---|---|---|---|---|---|
| 7.1 |  | Run a cell down, with the meter across it between bursts, then a middling level, checking the cell | Main | By `src/battery.rs`, under 3.00 V for five seconds it steps down, and under 2.90 V for a second it fades down the ceiling (120, 90, 60, 30, 1) and turns off in about 7 s. It steps back up at 3.30 V. All loaded voltages, so rest voltage will read higher, and all unmeasured | The roadmap's low-voltage item, on a real light |
| 7.2 |  | Compare a blinked-out voltage with the Fluke at three or four charge levels, off and on | T2 | Within 0.1 V. This is what tests the 1094 calibration, and the 4.03 ratio behind it, on this light | "Check against a multimeter", the open question on 1094 |
| 7.3 |  | Near the cut-off, off for a minute, then on | Main | It starts. The MP3432 needs 2.7 V on VIN to start and 0.8 V to keep going, and the 2.90 V cut-off only leaves 0.2 V over that, so a nearly flat cell might run and then not restart. A sag at turn-on is the thing to watch for | The 2.7 V start-up against the cut-off |
| 7.4 |  | Blink out the temperature at a stable room, three or four readings | T2 | Near the room, ±3 °C by the datasheet. There's no reference to hold it to, so this only finds gross errors | Part of the thermal item |
| 7.5 |  | Blink out several temperature readings, light off, then at a low, a middle and a high level | T2 | How far they scatter, one against the next | The sensor's noise with the light on |
| 7.6 |  | Top of the ramp until it steps down, with a thermometer on the head | A thermometer | It steps down before the head's painful, near 50 °C at the sensor, and doesn't go under level 50. 1Lumen's head peaked at 52 °C. Only after this does `ramp::CEILING` get raised, a step at a time | "Thermal regulation", the `ramp::CEILING` decision |
| 7.7 |  | Charge it over USB with the cell in, watching for the light or a lost level | Main | Nothing odd. A lost level means the charger pulled VCC down far enough to reset the chip | "Level survives charging" |

## Test builds

None exist yet, and they should stay on a throwaway branch, because they aren't the firmware. Build and debug each on the breakout first.

**T1, another level.** A build that comes on at a set level. The docs/ui.md examples expect the real one, so `cargo xtask hex` won't write it as it stands. It needs a small `xtask` change that skips the examples, keeps everything else and writes the CRC as usual, since a hex made some other way misses the CRC and caps the light. `cargo xtask flash <port> <file>` already takes any hex.

**T2, a number out.** A build that blinks a value out on the main LED, tens on long blinks and units on short, at a low level. One build per value is fine. Blink codes stay out of the real firmware.

**T3, a hang on demand.** A build that stops feeding the watchdog when the switch has been held for ten seconds, so a real bite can be seen once. **Breakout only.** It's the only deliberate reset anywhere in this plan, and it isn't going near the torch. The real firmware doesn't do this at start-up, by choice, because of what happened to the earlier torch.

**T4, power-down after a feed.** `watchdog::Feeder::power_down` in `src/watchdog.rs` returning `resting` alone, dropping the `&& !self.fed`. The simulator checks the firmware against that same function, so it follows without a change. It's the one test build that goes on the torch, after 0.15 passes, and only to find out if the idle in the real firmware can go. It has the real firmware's protection and nothing else changed.

**T5, no fuse check.** The real firmware with `src/main.rs` setting `fuses: false` in place of `!hw::fuses().ok()`. **Breakout only**, for this breakout's SYSCFG0 of `0xFF` (0.3), which caps the real firmware at level 50 for good and makes 0.11, 0.12 and 0.15 unanswerable here. Writing the fuse back would fix it too, but this plan writes no fuses, so the build changes instead. Use it in place of Main for 0.11 and 0.12, and with T4's change as well for 0.15. It needs two things first.

- **T1's `xtask` change.** `cargo xtask hex` won't write it, because the simulator runs docs/ui.md's `fault 2C` example, and a build that ignores the fuses doesn't cap.
- **Pin 14's divider in place.** Pin 14 on G is a cell the firmware can't read, which caps it at level 50 all the same.

The torch never gets T5. If its own fuses fail at 1.9, that's a stop, not a reason to skip the check.

## What this kit can't do

| Check | Why | What would unlock it |
|---|---|---|
| The deadline for a tick that stops while the light's on | It needs the 32 kHz oscillator to stop, and nothing here can stop it | Nothing on this bench. Only the simulator has run it |
| Off current to the µA | No mA or µA range | A 1 kΩ resistor in series with the cell, which gives 1 mV per µA on the 600 mV range. The firmware waking each tick will wobble it, and the 3 readings a second will average that |
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
| 0.1 | 2026-11-11 | main `64de466` | `found tinyAVR P:0D:0-3`, ten out of ten, none on a retry. The chip was still running seesaw |
| 0.2 | 2026-11-11 | main `64de466` | Saved to `backup/breakout`, no serial warning. Serial `51 54 46 53 33 20 EC 86 22 40`. A second backup the same day matched it byte for byte |
| 0.3 | 2026-11-11 | | Fails `Fuses::ok`. SYSCFG0 reads `0xFF` (factory `0xF6`, errata DS80000886C §3.1.1), so RSTPINCFG is `0b11`, which DS40002204A §6.10.4.5 calls reserved. UPDI works anyway. SYSCFG1 reads `0xFF` (SUT 64 ms). Everything else is factory, and LOCKBIT is `0xC5`, unlocked. So 0.11, 0.12 and 0.15 will show the level 50 cap on this chip. Fuses not written |
| 0.4 | 2026-11-11 | `9d3b7dc`, flashed from main `64de466` | 628 bytes, hex SHA-1 `c16cdc85d0606c2c24453b481dad4553e74aa6af`. `flash` warned it isn't a signed full image, then verified |
| 0.5 | 2026-11-11 | `9d3b7dc` | A few dozen clicks. Both LEDs followed every one together, pin 7 bright and SCL dim, with no missed clicks. At each of three power-ups only the SCL LED flashed briefly (PB0's pull-up through reset), and pin 7's stayed dark |
| 0.6 | 2026-11-11 | `9d3b7dc` | 19.79 to 19.80 kHz on SCL, against 19.61 kHz expected, so the clock runs about 0.9% fast, within the internal oscillator's tolerance. Vin 3.294 V. SCL 137 mV on, against 129.2 mV for duty 10 of 255, and 9 mV off. The 9 mV is the breakout's 10 kΩ pull-up on PB0 pushing about 0.33 mA into the pin's low driver, which also adds about 8.6 mV to the on reading (arithmetic). The torch's PB0 has no pull-up |
| 0.7 | 2026-11-11 | `9d3b7dc` | Pin 7 (PB2) read 0 V off and 3.293 to 3.294 V on, which is VCC, every time. 0 V through several power cycles by the Friend, never high without a click. The meter updates three times a second, so it can't rule out a shorter blip at power-up |
| 0.8 | 2026-11-11 | `9d3b7dc`, xtask from main `7a2a45e` | Three rounds of `peek` then `ping` with the light on, through the Friend. Every `peek` read `VPORTB.OUT` `04` and `RSTCTRL.RSTFR` `01`, and every `ping` said `found tinyAVR P:0D:0-3`. The SCL LED stayed lit through all of it. So no command reset the chip. PB2 survived each round, and RSTFR never gained UPDIRF (`0x20`), which `first-light` never clears. That's against the System Reset DS §33.3.2.2.2 has UPDIDIS issue. Either it doesn't happen here or `xtask`'s UPDIDIS write doesn't land, and this doesn't tell which. One chip, one adapter, so it says nothing yet about the torch's kit. The rest of the first `peek` matched `first-light`'s setup, with MCLKCTRLB `01`, WDT.CTRLA `00`, DIRB `05`, DIRC `0E`, TCA0 CTRLA `01` and CTRLB `17`, PITCTRLA `41`, SREG `02` |
| 0.9 | 2026-11-11 | main `a535534` | Signed image, 4,989 bytes of code. `flash` wrote and verified all 16,384 bytes with no warning. `restore` from `backup/breakout` matched the serial number first, then wrote flash and EEPROM and verified, so seesaw was back. `flash` again verified. Pin 14 was jumpered to G rather than on the divider, which doesn't matter to this row |
| 0.10 | 2026-11-11 | main `a535534` | No resistors yet, so pin 14 was jumpered to G, a cell the firmware can't read. With that and the 0.3 fuse fault, the light was held at level 50, as docs/ui.md says for both. 1C, 1H, 2H, 1H ramp, 2C and back, 4C lockout with 1H momentary and 4C unlock, and 3C and 5C doing nothing all behaved as the spec says, with brightness changes only showing below 50. A dozen power cycles came up dark every time, with no reset loop. It went off by itself at five minutes, as the can't-read-the-cell row says |
| 0.15 | 2026-11-12 | T4, main `89f6d27`'s firmware with `power_down` returning `resting` (stamp `ts26 f615acc3b0+`), and main (stamp `ts26 f615acc3b0`), both flashed through the kit | Without T5, so the 0.3 fuse fault and pin 14 on G held it at level 50 throughout, and the 51% check stays open. The remembered level answers the reset question on its own, since a reset puts it back at 52. On SCL, 2C read 172.7 mV (level 50) and 1H from off 15.2 mV (level 13). Clicked off at level 13 at 18:30 and left an hour. Then the first 1C turned it on, the second read 9 mV (off) and the third came back at 15 mV, level 13, so it remembered the level and didn't reset. The first two readings were lost to a lead off G. Off current on an iFixit CM-D20's mA range in series with Vin at the kit's 4.5 V, pin 14 on G, was 1.56 mA steady on main and 0.75 mA on T4, a 0.81 mA saving, about twice the 0.37 mA estimate. Likely the higher VCC, unmeasured. Both include the breakout's power LED and regulator. One hour on a breakout, so 6.4 on the torch still decides whether the idle goes |
| 0.16 | 2026-11-12 | main `a535534` on the chip | The kit's pins are marked +, − and R. Only − was continuous with the USB plug's shell, so − is ground. The breakout's green power LED flashed while the meter's lead was on +, so the continuity test current reached Vin through it. Plugged in, + read 4.5 V to −, and R 26 mV under +, so both idle high as expected. Wired + to Vin, − to G and R to UPDI (R read as Reset, since UPDI shares PA0 with RESET, which 0.17 settles). Vin 4.513 V, power LED lit, LEDs off the pins |
| 0.17 | 2026-11-12 | main `a535534` on the chip, xtask from main `89f6d27` | Leads off the breakout and touching nothing, "no reply from the chip", so the port and the kit's echo work. Wired as 0.16, `found tinyAVR P:0D:0-3`, the same as 0.1, so R is UPDI and the pin order is settled |
| 0.18 | 2026-11-12 | main `a535534` on the chip, xtask from main `89f6d27` | Saved to `backup/breakout-kit`, serial `51 54 46 53 33 20 EC 86 22 40` as in 0.2. `fuses.hex`, `serial.hex` and `eeprom.hex` match `backup/breakout` byte for byte, and `flash.hex` differs, since that one is seesaw. `backup` said the flash has no stamp, which is expected, because `a535534` predates the stamp (`d7fb06f`) |
| 0.19 | 2026-11-12 | `9d3b7dc`, flashed from main `89f6d27` | Hex SHA-1 `c16cdc85d0606c2c24453b481dad4553e74aa6af`, as 0.4. The same warning about an image short of 16,384 bytes, then 628 bytes written and verified. Pin 7 (PB2) read 1.5 mV before a click, 4.506 V on and 1.5 mV off again, on the kit's 4.5 V. Off current 5.89 mA, on an iFixit CM-D20's mA range in series with Vin, with the light off and the LEDs off the pins. That's the chip awake, since `first-light` never sleeps, plus the breakout's power LED and regulator, so it's only a baseline for comparing other builds on this board |
| 0.20 | 2026-11-12 | seesaw from `backup/breakout`, xtask from main `89f6d27` | `restore` matched the serial number `51 54 46 53 33 20 EC 86 22 40` to the one saved in `backup/breakout`, then wrote all 16,384 bytes of flash and the EEPROM and verified. `ping` then said `found tinyAVR P:0D:0-3`. Seesaw is back, and `restore` has now run through the kit that goes on the torch |
| 1.1 | 2026-11-12 | stock | 4.138 V out of the light, on an iFixit CM-D20 |
| 1.2 | 2026-11-12 | stock | Partly done, on an iFixit CM-D20 from cell negative to the tube. Off, about 2 mA at first, settling to 0.12 mA, on the mA jack. 1C, at whatever level stock remembered, 0.65 A on the A jack. The lowest level and the steps above it were skipped, since holding the probes on and pressing the switch at the same time didn't work. Clip leads would do it, before the stock firmware is replaced in stage 2 |
| 1.3 | 2026-11-12 | | Cell out, driver photographed from the battery side. Three round bare pads in a short row at the edge of the board, between the spring and the brass ring, at about 4 o'clock with the board's `TS26-A-S5` marking at 12. The silkscreen beside them reads `R - +`, with R by the pad nearest the end of the board's other marking, `20251008`, then − and + going away from it. Those are the same three marks as the kit's pins (0.16). The labels sit beside the pads on the spring side, so the pairing is by position, and it's the silkscreen's word only until 1.5. The `6` in `TS26-A-S5` is partly under a row of vias, and `20251008` was read by eye off the board, since vias blur its last digits in the photograph. No scale in the frame, so nothing on pitch, which is 1.4 |
| 1.4 | 2026-11-12 | | Checked against the kit's probe. Its three pins line up with the three pads, so the pitch is the one that probe is made for, 1.27 mm, and this looks like the common three-pad layout. Not measured with calipers, so there's no figure in millimetres. The kit's probe is the one to use, once 1.5 has named the pads |
| 1.5 | 2026-11-12 | stock | Cell out, on an iFixit CM-D20, pads named by their silkscreen. Only − was continuous with the battery negative contact. + to − started at about 1.8 MΩ and climbed to about 2 MΩ, and R to − read about 20 MΩ from the start, so + is the supply pad and R is UPDI, agreeing with the silkscreen. Diode mode against the battery positive contact, red lead on the pad first. + read OL, then 1.092 V reversed, which is the cell's path to the supply and expected. OL makes a path from the supply pad into the cell rail less likely, without ruling it out. R read OL, then 2.149 V reversed, which the plan didn't predict. Not a short, and well above a silicon junction's drop. 1.6's direct reading between + and R doesn't make it the + pad's path plus a diode |
| 1.6 | 2026-11-12 | stock | Cell out, on the same meter, with the red lead named first. Ohms, red on − to + read 50 kΩ, likely the supply rail's diodes conducting backwards, and red on + to − climbed fast, the capacitors charging as in 1.5. − and R read about 6 MΩ either way. Red on R to + started at about 6 MΩ, and red on + to R started at about 2 MΩ and climbed. So nothing near the 20 to 60 kΩ pull-up, which likely doesn't conduct unpowered, and no short. Diode mode, red on + to R read 1.872 V, and red on R to + 2.890 V. Neither is a clean junction's 0.5 to 0.7 V, which fits the plan's reasoning that PA0 has no clamp diode to the supply. That stays an inference from the pin's 13 V rating, since the datasheet never draws the pad, and its caution under table 36-1 says the pin has ESD protection of some kind (DS40002204A §36.2). iFixit doesn't publish the meter's diode test open-circuit voltage, so 2.890 V might be close to OL, and a second meter on DC volts across its probes would say. Nothing disagrees with 1.5's names |
