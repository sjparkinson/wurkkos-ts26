# The UI

What every click and hold does. This is the spec. `src/ui.rs` implements it and its tests check it, so change this file in the same commit as the code.

The notation is Anduril's. **1C** is one click, **2C** two quick clicks, **1H** press and hold, **2H** click then press and hold, and so on. A press becomes a hold after about 375 ms, and clicks count once the switch has been left alone for about 280 ms.

Every row has an example, which is a script in that notation, starting from a torch that's just had its battery put in, and how the light should be after it (`off`, `on` or `level N`). A hold lasts a second unless it says otherwise (`1H:3s`), and `wait:1s` leaves the switch alone. A script that ends in a hold never lets go, so it shows the light while it's still held. `cargo test` runs the examples against the library, so a wrong one fails. Only Planned rows can leave it as `-`. The background table at the end is the exception, since it isn't clicks.

## Off

| Action | Does | Example | Status |
|---|---|---|---|
| 1C | On, at level 52 on a fresh torch | `1C` → level 52 | Done |
| 1C, after using it | On, at the last level used | `1C 1H 1C 1C` → level 92 | Done |
| 1H | On at the lowest level. Keep holding and nothing more happens | `1H:3s` → level 13 | Done |
| 1H, then 1H again | Let go and hold again to ramp up | `1H 1H` → level 53 | Done |
| 2C | On at turbo | `2C` → level 120 | Done |
| 4C | Lockout | `4C 1C` → off | Done |
| Anything else | Nothing | `3C 5C 2H:2s wait:1s` → off | Done |

## On

| Action | Does | Example | Status |
|---|---|---|---|
| 1C | Off, remembering the level | `1C 1C` → off | Done |
| 1C at turbo | Off, and turbo's the level it remembers | `2C 1C 1C` → level 120 | Done |
| 1H | Ramp up, stopping at turbo | `1C 1H:3s` → level 120 | Done |
| 2H | Ramp down, stopping at the lowest level | `1C 2H:3s` → level 13 | Done |
| 2C | Turbo | `1C 2C` → level 120 | Done |
| 2C at turbo | Back to the level before turbo | `1C 1H 2C 2C` → level 92 | Done |
| 2C at a remembered turbo | Back to the last level before turbo | `2C 1C 2C 2C` → level 52 | Done |
| Anything else | Nothing | `1C 3C 4C 5C 3H wait:1s` → level 52 | Done |

The ramp moves one level per 64 Hz tick, so bottom to top takes about 1.7 seconds.

Turbo is `ramp::CEILING`, level 120 of 150, and nothing goes above it until thermal regulation has been checked on a real light. The lowest level is 13. The stock firmware's smooth floor on this light is 5 of 150, which is 36 in delta-sigma units, and level 13 is the first on this curve to reach that. Reviewers saw flicker below it on theirs. Nobody's measured it on this firmware.

The remembered level lives in RAM and never in EEPROM. The torch sleeps rather than losing power, and SRAM is kept through sleep (DS §11.2), so nothing needs writing. The cost is that a cell swap or any reset loses the level, and it comes back on at 52. USB charging should keep it, since the cell stays in, but that's likely and not confirmed.

## Lockout

For pockets and bags.

| Action | Does | Example | Status |
|---|---|---|---|
| 1H | The lowest level, for as long as you hold it | `4C 1H:2s` → level 13 | Done |
| 1H, let go | Off again | `4C 1H:2s wait:1s` → off | Done |
| 4C | Unlock, back to off | `4C 4C 1C` → level 52 | Done |
| Anything else | Nothing | `4C 1C 2C 3C 2H:2s wait:1s` → off | Done |

## When something's wrong

The light looks after the cell and itself whatever you've asked for. It never flashes to say so. It goes down gently, and the level you asked for comes back once it can.

These scripts have four more words. `cell:2.95V` is the cell's voltage from there on and `heat:80C` the temperature. `cell:0V`, like any voltage outside 1.00 to 4.40 V, is a cell the firmware can't read. `fault` starts a torch whose power-on checks failed (on the simulated chip, a fuse that isn't what the firmware expects), and `flash:corrupt` one whose flash doesn't match its checksum. `cargo test` runs these against the library, and `cargo xtask hex` runs them against the firmware on a simulated chip. Every threshold behind them is unmeasured on this light.

| When | Does | Example | Status |
|---|---|---|---|
| Low cell | Steps down | `cell:2.95V 2C wait:6s` → level 105 | Done |
| Low cell, for long enough | Steps down, then off | `cell:2.95V 2C wait:60s` → off | Done |
| Flat cell | Fades down the ramp, then off | `cell:2.80V 2C wait:10s` → off | Done |
| Flat cell, 1C again | On again at the very bottom | `cell:2.80V 2C wait:10s 1C` → level 1 | Done |
| Flat cell, 1C again, a second later | Off again | `cell:2.80V 2C wait:10s 1C wait:1.5s` → off | Done |
| A cell it can't read | Holds the light at level 50 | `cell:0V 2C wait:10s` → level 50 | Done |
| A cell it can't read, for five minutes | Off, in one step | `cell:0V 2C wait:301s` → off | Done |
| A cell it can't read, 1C again | On again, still held at level 50, for another five minutes | `cell:0V 2C wait:301s 1C` → level 50 | Done |
| A cell it can read again | Back up to the level you asked for | `cell:0V 2C wait:10s cell:3.8V wait:30s` → level 120 | Done |
| Hot | Fades down, no further than level 50 | `heat:80C 2C wait:120s` → level 50 | Done |
| Hot, at a level under 50 | Nothing | `heat:80C 1H:3s wait:120s` → level 13 | Done |
| Hot, then cool | Back up to the level you asked for | `heat:80C 2C wait:120s heat:25C wait:120s` → level 120 | Done |
| A failed check at power-on | Holds the light at level 50 until the cell comes out | `fault 2C` → level 50 | Done |
| Flash that doesn't match its checksum | The same as any failed check | `flash:corrupt 2C` → level 50 | Done |
| A failed check, at a lower level | Nothing | `fault 1H:3s` → level 13 | Done |

A failed check is flash that doesn't match its checksum, a fuse that isn't what the firmware expects, or the watchdog having reset the chip. The light still works, since one that won't come on tells you nothing.

A cell it can't read is a voltage under 1.00 V or over 4.40 V, which is also what an ADC that never answers looks like. After a second of that the light fades down to level 50 and stays there. With no reading there's no low-voltage cut-off, and this cell has no protection of its own, so after five minutes lit without one reading it can believe, the light goes out, in one step and with no fade. It doesn't lock out, for the same reason a failed check doesn't, so each 1C buys another five minutes at level 50. The five minutes only count while the light's on, and one good reading starts them again. Readings that come back at 3.30 V or more step the level back up, as they do after a low cell. Five minutes is a judgement, and unmeasured like every other number here.

## Always on, in the background

| What | Does | Status |
|---|---|---|
| Aux LEDs | Dim and steady, blue when off, red in lockout, out while the main LED's lit. They never flash | Done |
| Sleep when off | Powers down, and looks at the switch 64 times a second | Done |
| A tick that stops | With the light on, off within about a tenth of a second, and it stays off until the watchdog resets the chip or the cell comes out | Done |

The aux colours say which of off and lockout you're in, so a click that does nothing doesn't look like a dead torch. 1Lumen measured 0.22 mA with the aux on low in red, which would be about 1.9 Ah a year if that's what this draws. Nobody's measured it on this firmware.

None of this has met a real light yet, so this isn't firmware to carry. See the [roadmap](roadmap.md).
