# Review notes

Things to decide or check once the port from the TS21 is finished. Each has what I'd do, and none of them blocks the work. Delete this file when they're all settled.

## Hardware safety

- **A dead 32 kHz oscillator leaves the light on, unprotected.** The tick and the watchdog both run from it, so if it dies with the light on, the loop stops and the watchdog stops with it. The TS21's TCB0 deadline covered this and went out with the clock check. I'd add a small deadline that only turns the light off, without the old permanent lock-out. `hw::wait_for_tick` still returns `bool`, always true, until this is decided.
- **A dead ADC means no low-voltage cut-off.** The battery side caps at level 50 and never turns off, and this cell is unprotected, with a boost converter that keeps running down to 0.8 V. I'd add a timed shut-off for that state.
- **PA1's digital input buffer stays on.** The datasheet says to disable it on ADC inputs (§30.3.1.1), but `.claude/CLAUDE.md` says to leave PA1's settings alone. I'd disable it, since that adds no pull-up, and reword the rule.
- **Idling on watchdog-feed ticks costs a lot when off.** I asked for it because the datasheet doesn't say a `wdr` survives an immediate power-down, and if it doesn't, a resting torch resets over and over and could look dead. It costs about 0.37 mA on average, so about 3.2 Ah a year from a 5 Ah cell, more than the aux. I'd keep it for the first flashes, then test power-down after a feed on the bench and drop it if a resting torch doesn't reset. It's not fit for a torch left in a drawer until then.
- **Battery thresholds are unmeasured.** Step down below 3.00 V, off below 2.90 V (Anduril's figure), recover at 3.30 V, readings treated as real from 1.00 to 4.40 V. Below 2.90 V the ceiling steps 120, 90, 60, 30, 1, then off, over about 7 s, rather than cutting out.
- **Level 50 is Anduril's number on our curve.** `limits::FAULT_CEILING` and the thermal floor both use it, and it's about 3.6% duty here. It may not match Anduril's brightness.
- **`ramp::CEILING` is 120.** Raise it to `ramp::LEVELS` once thermal has been run on a real light.

## After a hang

- **"Capped until the cell comes out" has gaps.** With no marker, the cap only lasts for the boot that sees the watchdog flag. A later reset clears it, and a jump to the reset vector doesn't count as a hang. I think that's a fair trade for simpler code.
- **`selftest::hung` doesn't exempt a UPDI reset.** A watchdog bite just before a flash could cap fresh firmware at level 50 until the cell comes out. It's rare and easily cleared, so I'd leave it.

- **The first boot might cap itself for no real reason.** Flashing over Anduril without a real power-on reset can leave the watchdog flag set. A cell bounce with the BOD off might do the same, and nobody's read this light's `BODCFG`. The cell-out step in docs/flashing.md covers the first case, as long as VCC really collapses.
- **Three checks can cap a good torch at level 50.** `Fuses::ok` has never seen a real TS26's fuse bytes, the CRC has never been checked by real hardware, and neither has the watchdog's window. If a fresh flash won't go above level 50, suspect these before the torch. `cargo xtask backup` gives you the real fuses before the first flash.

## UI

- **Lowest level is 13, not 5.** Our level 5 sits in the range reviewers saw flicker. It's one constant, `ui::FLOOR`, and unmeasured. The DSM 36 figure behind it came from a summary of Anduril's `anduril.h` and isn't in docs/hardware.md, so it needs a sourced check.
- **Aux glows dim blue when off, dim red in lockout.** At the 0.22 mA in docs/hardware.md that's about 1.9 Ah a year, roughly three times the TS21's button LED, and that figure was measured on low in red. You may prefer it off by default. The steady lockout colour is something the `ui` agent added, so a click that does nothing doesn't look like a dead torch.
- **Turbo is remembered,** as on `fancy-ui`: click off at turbo and the next 1C comes back at turbo.
- **3C does nothing.** The voltage blink is out under "no magic flashes", which leaves no battery check at all.

## Bench plan

- **The Friend on the breakout.** docs/bench-plan.md still uses it on a bare ATtiny1616 breakout, never on the torch. That fits the rule as worded, but say if you want it off the bench altogether.
- **T3 hangs on purpose.** It's a throwaway build for the breakout only, so a real watchdog bite gets seen once. It's outside the firmware, and easy to drop.
- **Breakout pins are a guess.** PB2 on TX and PC1-3 on pins 11-13 come from Arduino numbering, labelled likely.

## Simulator and xtask

- **`peek` isn't denied in `.claude/settings.json`.** It only reads, but it drives real hardware, so you'll probably want it next to `ping`, `backup`, `flash` and `restore`. I can't edit that file, and agents shouldn't.
- **The simulator doesn't model chip resets.** So a watchdog bite is an error there, and the `Faults.hang` path and the panic handler never run on it. It doesn't model anything analogue either, or a real tick landing between `sei` and `sleep`.
- **`flash`'s model check only reads the file name.** It catches a `-0614` hex, a rename defeats it, and it refuses `backup/flash.hex`, pointing you at `restore`. Stamping the model into the image would be the real fix.
- **`backup` stays a fixed `backup/`.** The TS21 moved to a `<dir>` argument, which wasn't ported, so a second torch needs its `backup/` moved first.
- **`sim.rs` keeps its own register table,** a partial copy of `src/regs.rs`, ATDF-checked, so the model doesn't move with the firmware. That bends the "every address lives in regs.rs" rule.
- **The fake chip's SIB is a TS21 reading.** No TS26 has been read yet.
- **The `.rs` files came in with `cp` and `sed`,** which `.claude/CLAUDE.md` says not to do, so the lint hook skipped them. Clippy and fmt both pass on the result.

## Housekeeping

- **`main` is behind.** This session can't touch the shared checkout, so the work is on `worktree-ramp-contract`, and it needs one fast-forward at the end.
- **Add the new signing key to GitHub,** or commits signed since the switch away from 1Password show as unverified. `~/.ssh/config` line 2 still points `IdentityAgent` at 1Password, which matters if the new key replaces it for GitHub too.
- **The TS21's brick isn't recorded in its own docs.** Its docs stop before the flash, the dark start and the short, and still give a 1.27 mm pitch where you measured about 1.75 mm.
