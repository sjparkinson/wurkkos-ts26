# Hazard log

Each way found so far that this torch could hurt someone or itself, and what covers it. The code is the source of truth, so this only changes when a hazard is added or removed, or when its cover changes in kind.

### H1. The boost converter on with nothing controlling it

**Hazard.** PB2, the enable, floats from reset until firmware drives it and in programming mode (DS40002204A §16.3.1, §33.3.8.2), or is left high after a stray jump to the reset vector, or rises before the duty is set.

**Cover.** `main` calls `hw::hold_enable_low` first, which writes `OUT` before `DIR`. `hw::set_output` sets the duty then raises PB2, and drops PB2 then zeroes the duty. `hw::halt`, also the panic handler, turns off the same way. PB2 never gets a pull-up. A press from off runs PB0's PWM with PB2 low, and only lighting raises PB2.

### H2. A hang with the light on

**Hazard.** The loop stops with PB2 high and the PWM running.

**Cover.** A window watchdog (`hw::start_watchdog`, `watchdog::Pace`) resets the chip, and the next boot is a fault that won't light (`selftest::hung`). At rest the watchdog runs 8 s with no window, and the window is back two ticks after a press. If the 32 kHz oscillator stops with the PWM running, `hw::wait_for_tick` times out after about 104 ms and `main` calls `hw::halt`.

### H3. Over-discharge of an unprotected 21700

**Hazard.** The cell has no protection of its own, and the MP3432 pulls more current as it falls.

**Cover.** `battery::Protection` steps the ceiling down under 3.00 V, fades it off under 2.90 V, and latches flat until a reading with the light off is 3.60 V or more. Two lit readings in a row under 2.80 V turn it straight off. A light that turns on after an unloaded reading under 3.00 V is flat instead. At rest the torch reads the cell every five minutes, and the aux goes dark under `torch::AUX_DARK_BELOW`.

### H4. Overheating

**Hazard.** A lit light in still air, or with the head covered.

**Cover.** `thermal::Thermal` steps the ceiling down towards level 50 past 50 °C at the sensor, twice as fast above level 120. Nothing else holds the light under the top of the ramp.

### H5. Lit in a pocket or a bag

**Hazard.** A press on the switch under pressure.

**Cover.** 3C with the light on puts it out at the first click and locks it, and 3C in lockout unlocks it to off. A torch that's off only locks by lighting it first. In lockout `ui::Ui` never lights, a hold does nothing, and a fourth press makes a 4C, which does nothing.

### H6. Corrupt or half-written flash

**Hazard.** An interrupted or bad flash, or a flipped bit.

**Cover.** `hw::flash_ok` runs CRCSCAN over all of flash at power-on, and a failure is a fault. `flash` refuses a full image whose CRC doesn't check.

### H7. Wrong fuses

**Hazard.** Fuses the firmware's timing or its own protection don't expect.

**Cover.** `selftest::Fuses::ok` checks the fields the firmware relies on, and a failure is a fault. Nothing in this repo writes a fuse.

### H8. Losing UPDI or the chip while programming

**Hazard.** A fuse write that turns UPDI off, 12 V on the pad, a slipped probe or a wrong pad order. An earlier torch was lost on 2026-11-06 after a high-voltage UPDI programmer was hand-held on its pads.

**Cover.** `xtask/src/updi.rs` never writes fuses, the user row or a locked chip, refuses the wrong signature, and `restore` refuses a backup from another chip. Only the plain UPDI kit goes on the pads, never a high-voltage programmer, recovery included.

### H9. A stray interrupt

**Hazard.** An interrupt enabled by mistake, or with its flag already up, and no handler.

**Cover.** Every unused vector goes to `__unhandled_interrupt` in `src/avr/rt.rs` and on into `hw::halt`. PA5's interrupt is only on at rest, and `hw::rest` takes any wake that isn't a tick as a press.

### H10. A panic path in the firmware

**Hazard.** An overflow, an index or an unwrap reaches a panic.

**Cover.** `hw::halt` is the panic handler, and `cargo xtask hex` refuses an image with panic machinery linked in. Release builds keep overflow checks, and the library denies clippy's lints for indexing, casts and unwraps.

### H11. A stuck switch

**Hazard.** Dirt, a pressed-in switch or a jammed button.

**Cover.** `ui::Ui` bounds every hold. Locked, a hold does nothing. Off, 1H lights `ui::FLOOR` and stays there. Lit, it ramps and stops at `ramp::CEILING` or `ui::FLOOR`, and the ceilings in H3 and H4 still apply.

### H12. A dead or garbage ADC

**Hazard.** A broken ADC, a shorted or open PA1, or a reading from a rest.

**Cover.** `battery::plausible` (1.00 to 4.40 V) and `thermal::plausible` reject readings that can't be real. A run of bad voltages is a fault, and a bad temperature drops the thermal ceiling to its floor. Awake, the ADC also reads its own ground four times a second, and a run of results over `sensors::GROUND_MAX`, or none at all, is a fault.

### H13. A wrong clock

**Hazard.** A wrong `OSCCFG` frequency select, a locked calibration or a prescaler that isn't the one assumed.

**Cover.** `selftest::Fuses::ok` checks `FREQSEL` and `OSCLOCK`, and `hw::init` sets the prescaler.

### H14. A press read as more than one

**Hazard.** A switch that chatters, or that actuates twice on one press.

**Cover.** `button::Debounce` counts a change of the switch once two samples in a row show it. After a click that acts as it's let go, `button::Clicks` ignores the switch until it has been up for `button::RELEASE_TICKS`.
