# The UI

What every click and hold does. This is the spec. `src/ui.rs` implements it and its tests check it, so change this file in the same commit as the code.

The notation is Anduril's. **1C** is one click, **2C** two quick clicks, **1H** press and hold, **2H** click then press and hold, and so on. A press becomes a hold after about 375 ms, and clicks count once the switch has been left alone for about 220 ms. When more clicks wouldn't mean anything, the last click counts as it's let go, with no wait. That's 1C while it's on, and the third click of 3C while it's off. Lockout always waits, so you unlock with a pause after 3C.

Every row has an example, which is a script in that notation, starting from a torch that's just had its battery put in, and how the light should be after it (`off`, `on` or `level N`). A hold lasts a second unless it says otherwise (`1H:3s`), and `wait:1s` leaves the switch alone. A script that ends in a hold never lets go, so it shows the light while it's still held. `cargo test` runs the examples against the library, so a wrong one fails. Only Planned rows can leave it as `-`. The background table at the end is the exception, since it isn't clicks.

## Off

| Action | Does | Example | Status |
|---|---|---|---|
| 1C | On, at level 52 | `1C` → level 52 | Done |
| 1C, after using it | On at level 52 again, since nothing's remembered | `1C 1H 1C 1C` → level 52 | Done |
| 3C | Lockout, as the third click's let go | `3C 1C` → off | Done |
| 4C or more | Lockout, since 3C counts without waiting, and the clicks after it count in lockout, where they do nothing | `5C 1C` → off | Done |
| Anything else | Nothing | `2C 1H:2s 2H:2s wait:1s` → off | Done |

## On

| Action | Does | Example | Status |
|---|---|---|---|
| 1C | Off, as it's let go | `1C 1C` → off | Done |
| 1H | Ramp up, stopping at the top | `1C 1H:3s` → level 120 | Done |
| 1H again | Ramp the other way from the last 1H, stopping at the end | `1C 1H:3s 1H:3s` → level 13 | Done |
| 1H, after one that stopped partway | Still the other way, so letting go and holding again turns the ramp round | `1C 1H:1s 1H:500ms` → level 84 | Done |
| 1H, after turning it off and on | Up, since nothing's remembered | `1C 1H:3s 1C 1C 1H:3s` → level 120 | Done |
| More clicks, or a hold after clicks | The first click turns it off without waiting, and the rest count from off. So 2C is off and back on at level 52, 4C is off and locked, and 2H is off and then a hold that does nothing | `1C 1H:1s 2C` → level 52 | Done |
| 4C | Off, then locked | `1C 4C 1C` → off | Done |
| 2H | Off, then nothing | `1C 2H:2s` → off | Done |

The ramp moves one level per 64 Hz tick, so bottom to top takes about 1.7 seconds. Each 1H goes the other way from the one before it, whether or not that one reached the end, and the first after a click on goes up, since the light comes on near the bottom.

The top is `ramp::CEILING`, level 120 of 150, and nothing goes above it until thermal regulation has been checked on a real light. The lowest level is 13. The stock firmware's smooth floor on this light is 5 of 150, which is 36 in delta-sigma units, and level 13 is the first on this curve to reach that. Reviewers saw flicker below it on theirs. Nobody's measured it on this firmware.

## Lockout

For pockets and bags. The light never comes on.

| Action | Does | Example | Status |
|---|---|---|---|
| 3C, from off | Lock, with the aux bright red for five seconds | `3C` → off | Done |
| 3C | Unlock, back to off | `3C 3C 1C` → level 52 | Done |
| Any other click, or a hold | Nothing, and the aux bright red for five seconds from the click or the start of the hold | `3C wait:10s 2C` → off | Done |
| 1H | Nothing | `3C 1H:2s` → off | Done |
| Anything else | Nothing | `3C 1C 2C 4C 2H:2s wait:1s` → off | Done |

## When something's wrong

The light looks after the cell and itself whatever you've asked for. It never flashes to say so. Heat and a low cell take it down gently, and the level you asked for comes back once it can. A flat cell or a fault puts it out and keeps it out, and the aux LEDs say which.

These scripts have four more words. `cell:2.95V` is the cell's voltage from there on and `heat:80C` the temperature. `cell:0V`, like any voltage outside 1.00 to 4.40 V, is a cell the firmware can't read. `fault` starts a torch whose power-on checks failed (on the simulated chip, a fuse that isn't what the firmware expects), and `flash:corrupt` one whose flash doesn't match its checksum. `cargo test` runs these against the library, and `cargo xtask hex` runs them against the firmware on a simulated chip. Every threshold behind them is unmeasured on this light.

| When | Does | Example | Status |
|---|---|---|---|
| Low cell | Steps down | `cell:2.95V 1C wait:6s` → level 37 | Done |
| Low cell, for long enough | Steps down, then off, as a flat cell | `cell:2.95V 1C wait:60s` → off | Done |
| Flat cell | Fades down the ramp, then off, with the aux yellow for three seconds | `cell:2.80V 1C wait:10s` → off | Done |
| Flat cell, 1C again | Nothing, and the aux yellow for three seconds | `cell:2.80V 1C wait:10s 1C` → off | Done |
| Flat cell, rested but not charged, 1C | Nothing | `cell:2.80V 1C wait:10s cell:3.20V 1C` → off | Done |
| Flat cell, charged, 1C | On as usual | `cell:2.80V 1C wait:10s cell:3.80V 1C` → level 52 | Done |
| Flat cell, charged, left alone | No longer flat at the next check of the cell, within five minutes, so off's blue comes back with no press | `cell:2.80V 1C wait:10s cell:3.80V wait:301s` → off | Done |
| Low cell, at rest | Off's dim blue goes dark at the first reading under 3.30 V with the light off, which is a few ticks after power-on or within five minutes of rest. The light still comes on | `cell:3.20V wait:1s 1C` → level 52 | Done |
| Low cell, read by a press | Blue goes dark with the press, with no wait for a check | `wait:1s cell:3.20V 2C` → off | Done |
| Low cell, charged at rest | Blue again at the first reading of 3.40 V or more with the light off, within five minutes. One between the two changes nothing | `cell:3.20V wait:1s cell:3.80V wait:301s` → off | Done |
| A cell it can't read | Fades down the ramp, then off, as a fault | `cell:0V 1C wait:5s` → off | Done |
| A cell it can read again | Still a fault | `cell:0V 1C wait:5s cell:3.80V 1C` → off | Done |
| A cell it can't read, at rest | Nothing. The aux stays as it was, and it's no fault until a press reads it | `cell:0V wait:900s cell:3.80V 1C` → level 52 | Done |
| Hot | Fades down, no further than level 50 | `heat:80C 1C 1H:3s wait:120s` → level 50 | Done |
| Hot, at a level under 50 | Nothing | `heat:80C 1C 1H:500ms 1H:3s wait:120s` → level 13 | Done |
| Hot, then cool | Back up to the level you asked for | `heat:80C 1C 1H:3s wait:120s heat:25C wait:120s` → level 120 | Done |
| A failed check at power-on | A fault | `fault 1C` → off | Done |
| Flash that doesn't match its checksum | The same as any failed check | `flash:corrupt 1C` → off | Done |

A flat cell is one that's gone under 2.90 V at level 1, or under 3.00 V for long enough that the steps down reach level 1. After that the light stays off, and clicks and holds do nothing but light the aux yellow for three seconds. A press reads the cell before its click counts, with the light off, and if that reading is 3.60 V or more the click turns the light on as if the torch were new. That's a cell that's been charged or swapped, since one that cut out at level 1 rests only a little above where it cut, and a rested cell at 3.3 V is close to empty. It's one reading because a press only has time for one. The check every five minutes at rest (below) takes the same reading, so a flat cell that's been charged is good again within five minutes with no press, and off's blue comes back to say so. A charger lifts the cell's voltage while it runs, so a cell on charge can pass 3.60 V before it's taken much. So USB charging brings the light back without taking the cell out, and a flat cell can't be run at level 1 again with every click.

Every five minutes at rest, and on its first tick after the battery goes in, the torch reads the cell with the light off, the way a press does. It stays out of power-down for two ticks to do it, about 30 ms, and the reading's in on the third. A press or the light being on starts the five minutes again, since the cell's read then anyway, and a fault skips them. That reading decides off's dim blue. Under 3.30 V it goes dark, and it stays dark until a reading with the light off is 3.40 V or more, so a cell resting on the line doesn't flick it on and off. A press reads it too, before its click counts. Readings with the light on don't count, since the cell sags under load, and nor does one that comes in within two ticks of it going out. It's only the aux. The light still comes on with a low cell, and the cell's own thresholds above decide how far. A reading at rest that can't be real changes nothing. Each tick of rest forgets the ones before it, so they never add up to a fault at rest, and a dead ADC at rest goes unnoticed until a press reads it four times a second.

A fault is a failed check or a cell it can't read, and it lasts until the cell comes out. The light won't come on, and clicks and holds light the aux white for three seconds. A fault that comes up with the light on fades it down the ramp, a level a tick, so it doesn't snap out in your hand.

A failed check is flash that doesn't match its checksum, a fuse that isn't what the firmware expects, or the watchdog having reset the chip. So one watchdog reset leaves the torch dark until the cell comes out.

A cell it can't read is a voltage under 1.00 V or over 4.40 V, which is also what an ADC that never answers looks like. Three readings in a row like that go, and the fourth, about a second in, is a fault. With no reading there's no low-voltage cut-off, and this cell has no protection of its own, so a torch that can't read its cell doesn't light.

## Always on, in the background

| What | Does | Status |
|---|---|---|
| Aux LEDs | Steady. Dim blue when off, but dark while the cell last read under 3.30 V with the light off, until it reads 3.40 V or more. Bright red in lockout, for five seconds after a press. Out while the main LED's lit. Dim yellow for a flat cell and dim white for a fault, for three seconds after a press or after the light goes out. Dark otherwise. They never flash | Done |
| Sleep when off | Powers down, and looks at the switch 64 times a second | Done |
| Cell check when off | Stays out of power-down for two ticks after every five minutes of rest, and on the first tick after the battery goes in, to read the cell with the light off. Not with a fault | Done |
| A tick that stops | With the light on, off within about a tenth of a second, and it stays off until the watchdog resets the chip or the cell comes out | Done |

The aux colours say which of off and lockout you're in, so a click that does nothing doesn't look like a dead torch. On the T4 build, which powers down every tick at rest, the torch drew 0.12 mA off with the aux dark and 0.14 mA with dim blue (6.4 in [breakout.md](breakout.md#results)), so dim blue costs about 0.02 mA, at the meter's 0.01 mA resolution. Main idles a tick in three at rest and draws more off (4.5 read 0.58 mA with blue), but blue's share is the same on either. That's about 0.5 mAh a day, or about 4 % of a 4,265 mAh cell a year, which is why it isn't timed and only goes dark for a low cell. 1Lumen measured 5.4 mA with the stock firmware's aux on high, so bright red in a pocket would empty a full cell in about a month, and it only shows for five seconds. Dim red was hard to see on the light (4.6 in [breakout.md](breakout.md#results)), which is why lockout's is bright. The thresholds for blue are unmeasured, and the five minutes between checks are timed by the 32 kHz oscillator, so they're give or take a minute and a half. The channels are separate emitters that don't blend well, so off and lockout, the two you'll see most, get a channel each. A flat cell is yellow, red and green together, and a fault white, all three. Neither needs to blend to stand apart from the others. A flat cell and a fault only light the aux for three seconds at a time, since a cell that's already flat, or one the torch can't read, shouldn't be drained by the aux. The watchdog reset after a stopped tick is a failed check, so that torch shows white when pressed.

None of this has met a real light yet, so this isn't firmware to carry. See the [roadmap](roadmap.md).
