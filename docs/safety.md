# Hazard log

Each way found so far that this torch could hurt someone or itself, what covers it, and what's unproven. A hazard nobody has thought of isn't here. The code is the source of truth, so this only changes when a hazard is added or removed, or when its cover changes in kind.

- **simulator** means it has run on the fake chip (`cargo xtask hex`) or a host test checks it, which finds mistakes and proves nothing. CONTRIBUTING.md has what the simulator leaves out.
- **seen** means seen working on this torch ([hardware.md](hardware.md#seen-on-this-torch)).
- **measured** means a number from this torch backs it.

## Hazards

### H1. The boost converter on with nothing controlling it

**Hazard.** PB2, the enable, floats from reset until firmware drives it (DS40002204A §16.3.1) and in programming mode (§33.3.8.2), or is left high after a stray jump to the reset vector, or rises before the duty is set.

**Cover.** `main` calls `hw::hold_enable_low` first, which writes `OUT` before `DIR`. `hw::set_output` sets the duty then raises PB2, and drops PB2 then zeroes the duty. `hw::halt`, also the panic handler, turns off the same way. PB2 never gets a pull-up.

**Unproven.** Seen, as ten power-ups of the full firmware with no flash. With T0, one power-up on a bouncing contact gave a brief bright flash and another a red aux T0 never drives. The BOD is off and `SUT` holds code back 64 ms with every output tri-stated, so no firmware covers a power-up on a bouncing supply. The programmer's supply alone runs the converter. What holds PB2 low through reset, and the turn-on order against the MP3432's 180 µs, are unknown.

### H2. A hang with the light on

**Hazard.** The loop stops with PB2 high and the PWM running.

**Cover.** A window watchdog (`hw::start_watchdog`, `watchdog::Feeder`) resets the chip, and the next boot is a fault that won't light (`selftest::hung`). If the 32 kHz oscillator stops, `hw::wait_for_tick` times out off the PWM after about 104 ms and `main` calls `hw::halt`.

**Unproven.** Simulator. A bite has only been seen on the breakout. On the torch, an hour lit and several nights at rest saw none, on main and on T4. A spurious bite leaves the torch dark until a cell swap, which is the trade for never running on after a hang. A dead 32 kHz oscillator with the light off leaves the chip asleep with PB2 low, likely safe and unseen.

### H3. Over-discharge of an unprotected 21700

**Hazard.** The cell has no protection of its own, and the MP3432 pulls more current as it falls.

**Cover.** `battery::Protection` steps the ceiling down under 3.00 V, fades it off under 2.90 V, and latches `Protection::flat` until a reading with the light off is 3.60 V or more. The aux shows yellow briefly after a press, not for good. At rest the torch reads the cell every five minutes (`torch::CHECK_TICKS`), and off's dim blue goes dark under `torch::AUX_DARK_BELOW`. Lockout's bright red is timed (`torch::LOCKED_TICKS`).

**Unproven.** Simulator, with the readings scripted. Every threshold is unmeasured, and `battery::SLOPE` (1094) hasn't been checked against a meter. The MP3432's 2.7 V start-up leaves little over the 2.90 V cut-off. 3.60 V as a charged cell is a guess. Measured, dim blue costs about 0.02 mA and the torch draws 0.12 mA off on T4, which still drains a cell left for years.

### H4. Overheating

**Hazard.** A lit light in still air, or with the head covered.

**Cover.** `thermal::Thermal` steps the ceiling down towards `thermal::FLOOR` (level 50) past `thermal::LIMIT` (50 °C at the sensor). `ramp::CEILING` holds the light at level 120 of 150.

**Unproven.** Simulator, with the readings scripted. The sensor reads the die, and how it tracks the head is unknown. What level 120 draws, and how hot it gets, is unmeasured.

### H5. Lit in a pocket or a bag

**Hazard.** A press on the switch under pressure.

**Cover.** 3C locks and unlocks. In lockout `ui::Ui` never lights, a hold does nothing, and four presses are a 4C, which does nothing. Lockout is the only cover, since from off a hold lights `ui::FLOOR`.

**Unproven.** Seen, for locking, unlocking and the red. Nothing limits how long a lit light stays lit, and a cell swap unlocks it. 3C is fewer presses than the 4C it replaced. What the floor draws, and how warm it gets in a pocket, is unmeasured.

### H6. Corrupt or half-written flash

**Hazard.** An interrupted or bad flash, or a flipped bit.

**Cover.** `hw::flash_ok` runs CRCSCAN over all of flash at power-on, and a failure is a fault. `flash` refuses a full image whose CRC doesn't check.

**Unproven.** Simulator. The torch lighting shows the scan passed a good image, not that it fails a bad one. A bit flipped after power-on isn't noticed until the next.

### H7. Wrong fuses

**Hazard.** Fuses the firmware's timing or its own protection don't expect.

**Cover.** `selftest::Fuses::ok` checks the fields the firmware relies on, and a failure is a fault. Nothing in this repo writes a fuse.

**Unproven.** Seen to pass on this torch's factory fuses. `BODCFG` isn't checked, and the BOD is off here.

### H8. Losing UPDI or the chip while programming

**Hazard.** A fuse write that turns UPDI off, 12 V on the pad, a slipped probe or a wrong pad order. An earlier torch was lost on 2026-11-06 after Adafruit's High Voltage UPDI Friend was hand-held on its pads, and it then measured about 2.5 Ω between all three. 12 V or a slipped probe is the suspected cause, unproven.

**Cover.** `xtask/src/updi.rs` never writes fuses, the user row or a locked chip, refuses the wrong signature, and `restore` refuses a backup from another chip. Only the plain UPDI kit goes on the pads, never a high-voltage programmer, recovery included. The Friend puts 12 V on UPDI on every falling edge of RTS and nothing turns that off, and it shows as the same CH340 `1a86:7523` as the kit, so it stays unplugged and off the bench while the torch is open. The first flash on any torch is T0 (`9d3b7dc`, branch `build-t0`).

**Unproven.** Seen, for `ping`, `backup` and `flash` with the kit. `restore` has only run on the breakout. No test shows that no command writes a fuse.

### H9. A stray interrupt

**Hazard.** An interrupt enabled by mistake, or with its flag already up, and no handler.

**Cover.** Every unused vector goes to `__unhandled_interrupt` in `src/avr/rt.rs` and on into `hw::halt`, which `cargo xtask hex` checks.

**Unproven.** Simulator. Whether a pending interrupt that can't be taken wakes `halt`'s sleep is unknown.

### H10. A panic path in the firmware

**Hazard.** An overflow, an index or an unwrap reaches a panic.

**Cover.** `hw::halt` is the panic handler, and `cargo xtask hex` refuses an image with panic machinery linked in (`xtask/src/layout.rs`). Release builds keep overflow checks, and the library denies clippy's lints for indexing, casts and unwraps.

**Unproven.** Simulator at most. The check goes by symbol names, so `cargo xtask breakages` re-proves it after every nightly bump. Bits shifted out of the top and a truncating cast aren't machine-checked.

### H11. A stuck switch

**Hazard.** Dirt, a pressed-in switch or a jammed button.

**Cover.** `ui::Ui` bounds every hold. Locked, a hold does nothing. Off, 1H lights `ui::FLOOR` and the rest of that hold moves nothing. Lit, it ramps and stops at `ramp::CEILING` or `ui::FLOOR`, and the ceilings in H3 and H4 still apply.

**Unproven.** Seen, for ordinary holds while lit. 1H from off and a stuck switch are simulator only. A stuck switch lights the floor from off or from power-on and stays there, does nothing if it sticks within `button::RELEASE_TICKS` of a click that acted, and leaves a lit light at the level it reached. What the floor draws and how this switch bounces are unmeasured.

### H12. A dead or garbage ADC

**Hazard.** A broken ADC, a shorted or open PA1, or a reading from a rest.

**Cover.** `battery::plausible` (1.00 to 4.40 V) and `thermal::plausible` reject readings that can't be real. A run of bad voltages is a fault, and a bad temperature drops the thermal ceiling to its floor. Garbage at rest is forgiven.

**Unproven.** Simulator. A reading that's wrong but plausible and frozen isn't caught. A dead ADC at rest is only found at a press. 4.20 V has about 5 % to the top of `battery::PLAUSIBLE`, and the 1.1 V reference alone is ±2 % (DS40002204A table 36-23), so a light that reads high could call a full cell a fault. Seen, an hour lit reads as valid on this torch.

### H13. A wrong clock

**Hazard.** A wrong `OSCCFG` frequency select, a locked calibration or a prescaler that isn't the one assumed.

**Cover.** `selftest::Fuses::ok` checks `FREQSEL` and `OSCLOCK`, and `hw::init` sets the prescaler, which the simulator checks.

**Unproven.** Simulator, which only shows the firmware asks for the clock it models. On the breakout the clock read about 0.9 % fast. A power-on clock check was left out, since the oscillators' tolerances (tables 36-12, 36-13) leave a window that only catches a doubled or halved clock.

### H14. A press read as more than one

**Hazard.** A switch that chatters, or that actuates twice on one press. Lit, a 1C then reads as off and on again, at `ui::ON_LEVEL` whatever the level was.

**Cover.** `button::Debounce` counts a change of the switch once two samples in a row show it. After a click that acts as it's let go, `button::Clicks` ignores the switch until it has been up for `button::RELEASE_TICKS`.

**Unproven.** Simulator only, and not seen on this torch with or without the cover. How long this switch bounces is unmeasured, and chatter longer than a sample can still end a hold or split a click from off into two.

## What's left

- Nobody knows what holds PB2 low through reset and in programming mode (H1, H8).
- Every threshold is unmeasured, and no current against duty, temperature or top of the ramp has been measured (H3, H4).
- No low-voltage step, cut-off, fault or watchdog bite has been seen on this torch (H2, H3, H6, H12).
- `restore` has never run on this torch (H8).
- Nothing limits how long a lit light stays lit (H5, H11).
- Frozen but plausible readings get past both plausibility checks (H12).
- The switch's bounce is unmeasured (H11, H14).
