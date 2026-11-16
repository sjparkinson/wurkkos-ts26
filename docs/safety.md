# Hazard log

One section for every way identified so far that this torch could hurt someone or itself, what covers it, and what evidence there is that the cover works. It's a log of the current state. A section changes in the same commit as the code it describes, and a hazard nobody has thought of isn't in here, so a short log doesn't mean a short list.

The full firmware hasn't met a real TS26. The programmer and T0, the first-light build, have, on one torch on 2026-11-16 ([breakout.md](breakout.md#results), stages 1 to 3), and breakout.md is the plan that takes it further. So read each section's Status strictly.

- **code only** means the code does it and nothing has run it, not even on the simulator.
- **simulator** means the firmware has run it on the fake chip (`cargo xtask hex`), or a host test checks the library. The simulator is one reading of the datasheet, run against code written from that same reading, so a pass finds mistakes and proves nothing. [CONTRIBUTING.md](../CONTRIBUTING.md#the-simulator) lists what it leaves out.
- **bench** means it's been seen working on a real TS26.
- **measured** means a number from a real TS26 backs the constant.

Nothing is past simulator today, since the statuses are about the full firmware. Some sections also cite a run on the breakout's chip, which is an ATtiny1616 but not a torch, or of T0 on the torch, and none of those moves a status.

## Hazards

### H1. The boost converter on with nothing controlling it

**Cause.** PB2 (the enable) is a floating input from reset until firmware drives it, or is left high after a stray jump to the reset vector, or is raised before the duty is set.

**Mitigation.** `hw::hold_enable_low` is the first call in `main`, and writes OUT before DIR. `hw::set_output` sets the duty and then raises the enable, and drops the enable and then zeroes the duty. `hw::halt` (also the panic handler) goes through the same off order. `hw::init` never gives PB2 a pull-up.

**Evidence.** Simulator `found_lit` starts with PORTB dirty. The simulator turns PB2 high with no duty into an error, and so a PB2 that stays high with the duty at 0 for longer than the delta-sigma's longest gap. Tests `the_enable_goes_after_the_duty`, `a_dsm_gap_is_fine_but_a_longer_zero_duty_is_not`, `the_dangerous_pins_never_get_a_pull_up`, `power_down_wants_pb2_low` and `found_lit_wants_out_cleared_by_the_first_register_write` in xtask/src/sim.rs. Power-down with PB0 still on TCA0, or a duty left in CMP0 or CMP0BUF, is an error too (`power_down_wants_no_duty_waiting_no_conversion_running_and_the_pit_synced`). The firmware never asks to rest with the light on, so `prepare_sleep`'s guard against powering down with PB2 high is run by examples/guards.rs instead (`sim::guards`). On the breakout, T0 read 0 V on PB2 off and VCC on, through several power cycles (0.7), at 3 readings a second.

**Status.** Simulator.

**What's left.** Nobody knows what holds PB2 low on the torch's board, or if anything does (3.1 and 3.7). With the cell out, the UPDI kit's supply alone lit the emitters once as its probe landed on the pads (1.7), which fits the stock firmware's start-up blink. So the converter can run from the programmer. Programming mode, where PB2 floats for the whole command, stayed dark through a backup (1.8). With T0 on the torch, a bouncing contact as the cap went on gave one brief bright flash from the main emitters, and on another power-up the red aux, which T0 never drives (3.1). The BOD is off on this chip (1.10). `SUT` holds code back for 64 ms after reset with every output tri-stated (DS40002204A §12.3.2.2, §16.3.1), and POR releases at 1.4 to 1.8 V, under the 2.7 V that 10 MHz needs (tables 36-9 and 36-3), so a power-up on a bouncing supply likely isn't clean, and nothing in firmware runs early enough to cover either. About eight clean power-ups stayed dark through the same window, which is indirect evidence that something holds PB2 low. Reasoning, unconfirmed. The order against the MP3432's 180 µs needs a scope. The reset-to-`hw::hold_enable_low` gap isn't covered by any code, and the meter can't see it.

### H2. A hang with the light on

**Cause.** The loop stops, so PB2 stays high and the PWM carries on.

**Mitigation.** `hw::start_watchdog` sets a 32-clock window and `watchdog::Feeder` says which passes feed it. A tick that stops while the light's on has its own deadline, `watchdog::TICK_DEADLINE_CYCLES` (2048 PWM cycles, about 104 ms), after which `main` calls `hw::halt`. The next boot after a bite is a fault and won't light (`selftest::hung`).

**Evidence.** Simulator `slow_ticks` runs every tick as slow as the datasheet allows, and `oscillator_stops` stops the 32 kHz oscillator with the light on. Tests `every_feed_lands_mid_window` and `the_tick_deadline_is_104_ms_against_ticks_of_306_cycles_or_460_at_worst` in src/watchdog.rs, and `a_feed_in_the_open_window_is_fine_and_a_closed_one_bites` and `a_stopped_oscillator_stops_the_pit_and_the_watchdog` in xtask/src/sim.rs. On the breakout, a build that stops feeding after a ten-second hold went out, and the next boot was capped at level 50 until a power cycle (breakout.md 0.13).

**Status.** Simulator.

**What's left.** Nobody has seen the watchdog bite on a torch, only on the breakout's chip (0.13). Since a bite is a fault, one spurious bite, from a window that's off on a real chip for instance, leaves the torch dark until a cell swap, where it used to leave a dim light. That's the trade for never running on after a hang. The simulator has no resets, so a bite is a failed run and what follows one never runs. The watchdog and the tick share an oscillator, which is why the deadline counts PWM cycles off the other one. [breakout.md](breakout.md) T4 is still open, so `main` idles on the tick after a feed and costs about 0.37 mA at 3 V, or about 0.6 mA at 4.2 V (a third of the typical idle current in DS40002204A table 36-4, interpolated between its 3 V and 5 V rows). A dead 32 kHz oscillator with the light off leaves the chip asleep with PB2 low, which is probably the safe state and nobody has seen.

### H3. Over-discharge of an unprotected 21700

**Cause.** The Wurkkos cell has no protection of its own, and the MP3432 pulls more current as the cell falls.

**Mitigation.** `battery::Protection` steps the ceiling down below `battery::STEP_DOWN_BELOW` (3.00 V) and fades it down and off below `battery::OFF_BELOW` (2.90 V). A rest keeps the ceiling (`Protection::rest`). Turning off for a flat cell latches `Protection::flat`, and the light stays off until a reading with it off is `battery::CHARGED_AT` (3.60 V) or more, so clicking can't run a flat cell at level 1 again and again. The aux shows yellow for three seconds after a press, then goes dark, so it doesn't drain the cell either (`torch::SHOW_TICKS`). At rest the torch reads the cell after every `torch::CHECK_TICKS` (five minutes), and on its first tick after power-on, through `Limits::tick` with the light off as a press does, so a flat cell that's been charged reads as good again without a press. Off's dim blue goes dark while the last reading with the light off was under `torch::AUX_DARK_BELOW` (3.30 V), until one is `torch::AUX_LIT_FROM` (3.40 V) or more, so a cell left in a drawer stops paying for it once it's low. It isn't timed, since on this torch it costs about 0.02 mA (0.14 mA off with it lit against 0.12 mA dark, on the T4 build, breakout.md 6.4), about 0.5 mAh a day. Lockout's bright red shows for five seconds after a press (`torch::LOCKED_TICKS`), since 1Lumen's 5.4 mA for the stock firmware's bright aux would flatten a full cell in about a month. A cell it can't read is a fault (H12), but only from readings a press or the light takes. `Protection::rest` forgives the glitch count on every tick of rest, so the one reading a check takes never adds to it.

**Evidence.** `a_low_cell_steps_down_gradually_then_turns_off`, `a_flat_cell_fades_down_the_ramp_before_it_turns_off` and `a_flat_cell_stays_off_until_it_reads_as_charged` in src/limits.rs. `a_flat_cell_turns_off_shows_yellow_and_ignores_clicks_until_charged` in src/torch.rs. `lockout_shows_bright_red_for_five_seconds_after_each_press` and `a_hold_in_lockout_cannot_keep_the_red_lit` in src/torch.rs. For the check at rest, in src/torch.rs, `the_cell_is_read_on_the_first_tick_and_after_every_five_minutes_of_rest`, `a_press_or_the_light_starts_the_five_minutes_again`, `blue_stays_lit_through_three_days_of_rest_with_a_good_cell`, `a_low_cell_at_power_on_darkens_blue_from_the_first_check`, `blue_goes_dark_under_3_30_volts_and_comes_back_at_3_40`, `readings_with_the_light_on_never_move_the_aux`, `a_flat_cell_charged_at_rest_is_good_again_at_the_next_check`, `garbage_at_rest_leaves_the_aux_alone_and_is_no_fault`, `a_fault_skips_the_checks` and `random_switching_with_long_rests_never_breaks_a_promise`, and in src/limits.rs `an_unloaded_reading_is_one_with_the_light_out_for_two_ticks_before_it` and `a_check_at_rest_seeds_the_filter_afresh_and_leaves_the_ceiling`. `flat_stays_flat_until_a_reading_with_the_light_off_says_its_charged` in src/battery.rs. In src/battery.rs, `steps_down_after_5_seconds_below_3_volts`, `a_second_below_2_9_volts_drops_a_big_step_rather_than_turning_off`, `turns_off_after_a_second_below_2_9_volts_at_level_1`, `a_long_rest_keeps_the_ceiling_and_forgets_the_filter`, and `every_reading_held_ends_where_the_thresholds_say`, which holds every reading the ADC can give. docs/ui.md's `cell:2.95V`, `cell:2.80V` and `cell:3.20V` rows, run by the simulator.

**Status.** Simulator.

**What's left.** Every threshold is unmeasured. `battery::SLOPE` (1094) is the stock firmware's figure for another sample and hasn't been checked against a multimeter (7.2). The 2.7 V the MP3432 wants to start leaves little over the 2.90 V cut-off (7.3). Charging the cell down on purpose hasn't been done (7.1). Nobody knows what a cell that's cut out at level 1 rests at, so 3.60 V as the sign of a charged cell is a guess, and it rests on one reading, since a press has time for no more. Too low and a flat cell lights again on a click, too high and a part-charged one stays dark. Dim blue's 0.02 mA is one meter reading against another at 0.01 mA resolution, on the T4 build (breakout.md 6.4), with the meter and range not noted. Nobody's measured bright red on this firmware either, so the five seconds rest on 1Lumen's stock figure (5.4 mA). The 3.30 V and 3.40 V for blue are unmeasured too. A dead ADC at rest goes unnoticed until a press, and until then blue says nothing about the cell. The torch's own draw off, 0.12 mA on the T4 build (about 1 Ah a year) and more on main while it idles a tick in three, still drains a cell that's left for years, and nothing short of taking the cell out stops that.

### H4. Overheating

**Cause.** A lit light in still air, or a head that's covered.

**Mitigation.** `thermal::Thermal` steps the ceiling down towards `thermal::FLOOR` (level 50) as the smoothed and predicted temperature passes `thermal::LIMIT` (45 °C at the sensor). `ramp::CEILING` holds the light at level 125 of 150, a step up from the level 120 that the torch faded smoothly from (7.6).

**Evidence.** `a_hot_head_fades_down_to_the_floor` and `turning_on_resets_thermal_regulation` in src/limits.rs. In src/thermal.rs, `sustained_heat_takes_it_to_the_floor_and_no_further`, `the_top_settles_near_the_limit_without_hunting`, `turning_on_hot_steps_down_from_the_first_readings`, and `every_reading_held_ends_where_the_limit_says`, which holds every sum the ADC can give. `random_readings_never_lift_the_level_over_the_ui_or_the_cap` in src/limits.rs runs a million ticks of random readings. docs/ui.md's `heat:80C` rows, run by the simulator.

**Status.** Simulator.

**What's left.** Most constants are unmeasured. On one TS26, with the limit at 50 °C, the fade began with the outside of the head at about 45 °C (7.6), so the sensor likely reads about 5 °C over the outside of the head. That's one reading, and 45 °C hasn't been run on the light yet. The thermal tests' head model is a guess too, and above level 125 it warms past the limit by more than the tests allow before regulation catches it, so each step up in `ramp::CEILING` waits on 7.6 at the level before. A reading that's plausible and frozen goes undetected (H12). Nobody knows what level 125 draws (5.6).

### H5. Lit in a pocket or a bag

**Cause.** A click on the switch under pressure.

**Mitigation.** The 3C lockout (docs/ui.md). In lockout `ui::Ui` never lights, and answers only to 3C, which unlocks it. A hold from off does nothing either, so a switch pressed and held in a bag stays dark. Clicks count as they're let go when no more would mean anything (`ui::Ui::most_clicks`), but never in lockout, so four presses there are a 4C, which does nothing, rather than 3C to unlock and 1C to light. The aux shows bright red for five seconds after a press, so a locked torch is easy to tell from an off one, and a held switch can't keep it lit.

**Evidence.** `lockout_never_lights_and_3c_unlocks`, `off_answers_to_1c_and_3c_and_nothing_else`, `off_leaves_a_locked_torch_locked` and `aux_is_blue_when_off_red_in_lockout_and_out_with_the_light_on` in src/ui.rs. `lockout_never_lights`, `four_clicks_in_lockout_neither_unlock_it_nor_light_it`, `a_switch_held_from_power_on_stays_off` and `every_example_in_docs_ui_md_holds` in src/torch.rs. The Lockout rows of docs/ui.md, run by the simulator.

**Status.** Simulator.

**What's left.** Nothing limits how long a lit light stays lit, so a light that's on in a bag runs until the cell or the heat stops it. Lockout lives in RAM, so a cell swap drops back to off, unlocked (a bite is a fault, so that torch won't light anyway). 3C is fewer presses than the 4C it replaced, which makes an accidental unlock a little likelier, and nobody's tried the switch's actual feel under pressure.

### H6. Corrupt or half-written flash

**Cause.** An interrupted or bad flash, or a bit that's flipped.

**Mitigation.** `hw::flash_ok` runs CRCSCAN over all of flash at power-on and `selftest::Faults::flash` is a fault, so the light won't come on (`limits::Health::Fault`) and the aux shows white after a press. `crc::sign` puts a stamp of the commit just before the CRC in the last two bytes, so `backup` shows which build a torch has, and `cargo xtask hex` writes only that. `signed()` in xtask/src/main.rs makes `flash` refuse a full image whose CRC doesn't check.

**Evidence.** `the_scan_passes_a_signed_image_and_fails_a_changed_one` in xtask/src/sim.rs. In xtask/src/crc.rs, `matches_the_crc_16_ccitt_false_check_value`, `a_signed_image_fills_flash_and_ends_in_its_checksum` and `the_whole_of_a_signed_flash_checks_to_zero`. `flash_takes_only_a_signed_image` in xtask/src/main.rs. `a_failed_self_test_never_lights` and `random_readings_never_light_a_failed_self_test` in src/limits.rs. `a_fault_never_lights_and_shows_white_after_power_on_and_a_press` in src/torch.rs. docs/ui.md's `flash:corrupt` row.

**Status.** Simulator.

**What's left.** The scan and the CRC convention have only met the simulator's reading of the datasheet (6.1). A flipped bit after power-on isn't noticed until the next power-on.

### H7. Wrong fuses

**Cause.** A torch with a fuse the firmware's timings or its own protection don't expect.

**Mitigation.** `selftest::Fuses::ok` checks `WDTCFG`, `OSCCFG` (frequency select and `OSCLOCK`), `SYSCFG0.RSTPINCFG`, `APPEND` and `BOOTEND`, with `hw::fuses` only ever reading them. A failure is a fault, so the light won't come on. Nothing in this repo writes a fuse.

**Evidence.** `factory_fuses_pass`, `checks_fields_not_whole_bytes` and `each_wrong_field_fails` in src/selftest.rs. `only_a_fault_counts` in src/selftest.rs. docs/ui.md's `fault` row, run by the simulator. The check failed on the breakout's chip, whose `SYSCFG0` reads 0xFF (0.3), and the full firmware of the time then held at level 50, though pin 14 was grounded at the time too, so that run can't separate the two causes (0.10).

**Status.** Simulator.

**What's left.** Nobody has read a TS26's fuses (1.9). A bad fuse is the one thing in this log that's permanent, which is why nothing here writes one. `WDTCFG` and the reset pin are checked and `BODCFG` isn't (1.10).

### H8. Losing UPDI or the chip while programming

**Cause.** A wrong `SYSCFG0` write, 12 V on the pad, a slipped probe, or a wrong pad order.

**Mitigation.** The programmer never writes fuses, the user row or a locked chip, and refuses a chip with the wrong signature. No high-voltage programmer ever goes on this torch, recovery included. The first flash on any torch is T0 (commit `9d3b7dc`), and the full firmware goes on only after that's proven on the light. `restore` refuses a backup from another chip.

**Evidence.** `refuses_another_chip` and `refuses_a_locked_chip` in xtask/src/updi.rs. `restore_refuses_a_backup_from_another_chip_and_writes_nothing` in xtask/src/main.rs. [flashing.md](flashing.md) and [breakout.md](breakout.md) have the rules and the order.

**Status.** Code only.

**What's left.** No test shows that no command path writes a fuse. It's true by reading the code, and `nvm_commands_are_the_datasheets` in xtask/src/updi.rs only pins the command numbers. The pads on one TS26 have been found and their `R - +` silkscreen read (1.3), and their pitch fits the kit's probe (1.4). The meter named them ground, supply and UPDI as the silkscreen says (1.5 and 1.6). On 2026-11-16 the kit pinged, backed up and flashed T0 onto that torch, and it still talked afterwards with the pads reading as before (1.7 to 3.8). That's one torch and one adapter, and the full firmware's 16 KB flash hasn't run on it yet.

### H9. A stray interrupt

**Cause.** An interrupt that something enables by mistake, or whose flag was already up, with a vector that does nothing.

**Mitigation.** `__unhandled_interrupt` in src/avr/rt.rs zeroes r1 and jumps into `hw::halt`, which turns interrupts off, drops PB2, zeroes the duty and powers down. A `reti` would instead have returned with the flag still up, so the loop would crawl one instruction per interrupt with PB2 possibly high. Nothing enables any interrupt but the PIT and TCA0's overflow.

**Evidence.** The simulator makes it an error to enable an interrupt when its vector goes to `__unhandled_interrupt`. `vectors` in xtask/src/layout.rs insists the vector is `eor r1, r1` and a `jmp` to exactly `hw::halt` (`accepts_an_unhandled_interrupt_that_clears_r1_and_jumps_to_halt`, `rejects_an_unhandled_interrupt_that_doesnt_clear_r1_and_jump`). The simulator run `unhandled_interrupt` forces one on a lit torch and checks it ends dark in power-down. Stopping for good with SREG.I still set is an error, so `hw::halt`'s `cli` can't go missing (`stopping_for_good_wants_sreg_i_clear_too`), and so is turning TCA0's overflow on with its flag still up from before (`the_overflow_goes_on_with_its_flag_cleared_first`).

**Status.** Simulator.

**What's left.** The forced interrupt is a one-shot stand-in. A real stray one stays enabled and flagged, and the datasheet doesn't say whether a pending interrupt that can't be taken wakes `halt`'s sleep. PB2 is low before the first sleep either way, so the worst case is the chip spinning dark until the watchdog resets it, about 63 ms later, if the 32 kHz oscillator runs. Nobody has seen the watchdog bite.

### H10. A panic path in the firmware

**Cause.** An overflow, an index or an unwrap in code that reaches a panic.

**Mitigation.** `hw::halt` is the panic handler (`panic` in src/main.rs). It turns the light off, powers down, and leaves the watchdog to reset the chip. `cargo xtask hex` refuses an image that links any panic machinery (`no_panics` in xtask/src/layout.rs), so today nothing can reach it.

**Evidence.** `accepts_symbols_with_no_panic_machinery` and `rejects_a_panic_handler_or_a_bounds_check` in xtask/src/layout.rs. A forced out-of-bounds index in the loop made `cargo xtask hex` fail, naming `rust_begin_unwind`, `panic_bounds_check` and `panic_fmt`.

**Status.** Simulator.

**What's left.** The check matches symbol names, which can change with the nightly, so it has to be re-proven on every bump (CONTRIBUTING.md). src/lib.rs denies clippy's `arithmetic_side_effects`, `indexing_slicing`, `cast_possible_truncation`, `cast_sign_loss`, `cast_possible_wrap`, `as_conversions`, `unwrap_used`, `expect_used`, `panic` and `unreachable` in the library, and src/main.rs denies the same in the AVR build, which covers src/avr. So each site either saturates, checks, or carries an `#[expect]` saying why it can't overflow. Release builds have `overflow-checks = true` (Cargo.toml), so any `+`, `-`, `*` (compound forms included), negation or shift by too many bits that LLVM can't prove fits links a panic and `no_panics` fails the build. That machine-checks, for the whole firmware and not only the library, that none of those wraps unless it says `wrapping_*`. Bits shifted out of the top (`centivolts << 4` in `battery::Protection::filter`, for one) are never checked, and nor is a truncating `as` cast. Casts rest on the clippy denies above, and lost shift bits on reading. An unprovable `u8 + 1` added to `gradual::Gradual::tick` made `cargo xtask hex` fail, naming `rust_begin_unwind`, `panic_const_add_overflow` and `panic_fmt`. It proves no wrap, and nothing more. A value that saturates or clamps where it shouldn't still passes, and the `#[expect]` reasons for why those never trigger are read, not machine-checked.

### H11. A stuck switch

**Cause.** Dirt, a pressed-in switch, or a jammed button.

**Mitigation.** The UI's holds are bounded. From off, a 1H lights level 13 and nothing more happens however long it's held (`ui::Ui` swallows the hold). With the light on, a held switch ramps up and stops at `ramp::CEILING`, and both ceilings (H3, H4) still apply to the level it asks for.

**Evidence.** `hold_from_off_is_the_floor_and_does_not_ramp`, `ramps_stop_at_the_ends` and `a_hold_that_outlasts_off_does_not_light_it_again` in src/ui.rs. `the_ramp_never_asks_for_more_than_the_ceiling`, `a_switch_held_from_power_on_stays_at_the_floor` and `a_switch_held_while_on_still_answers_to_heat` in src/torch.rs. The `1H:3s` rows of docs/ui.md, and `switch_stuck` (twenty thousand ticks held from power-on), run by the simulator.

**Status.** Simulator.

**What's left.** A stuck switch keeps the light on at the level it reached, because nothing times it out (H5). Nobody has measured how short a press the switch gives, or how it bounces.

### H12. A dead or garbage ADC

**Cause.** A broken ADC, a shorted or open PA1, or a reading from a rest.

**Mitigation.** `battery::plausible` (1.00 to 4.40 V) and `thermal::plausible` reject readings that can't be real. `battery::GLITCHES` (three) are let go. After that a garbage voltage is `battery::Verdict::Blind`, which is a fault, so the light fades down the ramp and stays off until the cell comes out, with the aux white after a press. A garbage temperature drops the thermal ceiling to its floor and does no more. The check of the cell at rest every five minutes (H3) takes one reading between rests, and `Protection::rest` forgives the glitch count on every tick of rest, so garbage at rest is never a fault and leaves off's blue as it was.

**Evidence.** `a_dead_adc_is_a_fault`, `no_result_at_all_is_garbage_too`, `garbage_voltages_fade_it_out_and_latch_a_fault`, `garbage_voltages_with_the_light_off_are_a_fault_too`, `a_glitch_or_three_is_no_fault`, `a_garbage_temperature_alone_never_turns_it_off` and `garbage_from_power_on_shows_nothing_from_off` in src/limits.rs. `a_cell_it_cant_read_fades_it_out_and_shows_white`, `garbage_at_rest_leaves_the_aux_alone_and_is_no_fault` and `a_check_whose_result_never_comes_still_ends` in src/torch.rs. In src/battery.rs, `implausible_samples_never_reach_the_filter` and `a_second_of_garbage_is_blind_on_or_off`. In src/thermal.rs, `single_glitches_are_ignored`, `a_jump_that_lasts_is_believed` and `a_broken_sensor_steps_down_to_the_floor`. `a_result_with_no_conversion_started_is_ignored` in src/sensors.rs. docs/ui.md's `cell:0V` rows, and `adc_never_answers` (an ADC that never finishes a conversion), run by the simulator. The breakout ran the old can't-read-the-cell path, capped at level 50 and off at five minutes, on a real chip with PA1 grounded (0.10, 0.12a). Nobody has run the fault that replaced it on a real chip.

**Status.** Simulator.

**What's left.** A reading that's plausible and frozen isn't detected, so a stuck ADC that shows a good voltage looks like a good cell. A garbage temperature never turns the light off, only holds it at the floor, which is a choice and unmeasured. A dead ADC at rest goes unnoticed until a press or the light reads it, so blue keeps whatever the last good reading said. A second of garbage voltages leaves the torch dark until a cell swap, so an ADC that misbehaves once costs the light, which is the trade for never running a cell blind. The top of `battery::PLAUSIBLE` sits close to a full cell. A 4.20 V cell has about 5 % to the ADC's top, and the 1.1 V reference alone is ±2 % in this torch's range and ±5 % over the chip's full one (DS40002204A table 36-23, characterised, not production tested), before the divider's tolerance and the 1094 calibration. A light that reads high could call a full cell garbage, and so a fault (breakout.md 7.8). `a_plausible_voltage_that_never_changes_keeps_the_light_on` and `a_plausible_temperature_that_never_changes_never_steps_down` in src/limits.rs pin that gap down rather than close it.

### H13. A wrong clock

**Cause.** A wrong `OSCCFG` frequency select, a locked oscillator calibration, or a prescaler that isn't the one assumed.

**Mitigation.** `selftest::Fuses::ok` checks `FREQSEL` and `OSCLOCK`. The prescaler is the firmware's own write in `hw::init` (`CLKCTRL_MCLKCTRLB`, divide by 2), and the simulator checks the clock it ends up with (`clocked_as_modelled` in xtask/src/sim.rs). `watchdog::CLOCK_HZ` is that clock, and the tick deadline's sums start from it. On the breakout, T0's PWM read 19.79 to 19.80 kHz against 19.61 expected, so about 0.9 % fast (0.6).

**Evidence.** `each_wrong_field_fails` in src/selftest.rs and `clocked_as_modelled`. The breakout's frequency reading, which is T0 on a different chip.

**Status.** Simulator.

**What's left.** A power-on clock check was considered and left out. DS40002204A tables 36-12 and 36-13 give OSC20M as ±5 % and OSCULP32K as ±30 % over the full operating range, characterisation and not production tested. Their ratio can be +50 % or −27 % on a good chip, so a window that never fails a good part only catches a clock that's doubled or halved, which the fuse check and the simulator already cover. A dead OSC20M stops the CPU, and a dead OSCULP32K is H2's deadline.

## What's left

This is true of every hazard, so it's kept out of the sections above.

- **Nearly every threshold is unmeasured.** The cell's cut-offs, the level 125 ceiling, the 3.60 V that clears a flat cell, the three seconds of aux, the 104 ms and the second of garbage that makes a fault are all from the stock firmware, the datasheet or the project's own judgement, and none has been checked on a TS26. The temperature limit has been seen working once on one, at its old 50 °C (7.6 in breakout.md).
- **The simulator's limits** are in [CONTRIBUTING.md](../CONTRIBUTING.md#the-simulator). The two that matter most here are that it has no chip resets, so a watchdog bite and the boot after it never run, and that it models nothing analogue, so the driver, the cell and the head are all scripted.
- **Frozen but plausible readings** get past both `battery::plausible` and `thermal::plausible`. Nothing in the firmware can tell a stuck reading from a steady one, and two tests in src/limits.rs pin that down so a change to it shows.
- **There's no run-time limit.** A lit light with a good cell and a cool head stays lit, whatever pressed the switch.
- **[breakout.md](breakout.md) is what would move hazards to bench.** Stage 3's T0 run is the gate for H1 and H8, stage 6 for H2, H6 and H7, and stage 7 for H3, H4 and H12's thresholds. Until a hazard has a result there, it stays where it is.
