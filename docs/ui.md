# The UI

What every click and hold does. This is the spec. `src/ui.rs` implements it and its tests check it, so change this file in the same commit as the code.

The notation is Anduril's. **1C** is one click, **1H** press and hold, and so on. Clicks count once the switch has been left alone for about 280 ms.

For the first flash the UI is as small as it gets, so that when something doesn't work it's the hardware or the toolchain, not the UI.

## Off

| Action | Does | Status |
|---|---|---|
| 1C | On, at the one dim level | Done |
| Anything else | Nothing | Done |

## On

| Action | Does | Status |
|---|---|---|
| 1C | Off | Done |
| Anything else | Nothing | Done |

"On" means the boost converter enabled (PB2 high) with a PWM duty of 10 of 255 on PB0. What that comes out as in lumens nobody knows yet, since the MP3432's response to PWM on this board hasn't been measured. Off means PB2 low and the duty at 0. The aux LEDs stay off.

## Always on, in the background

| What | Does | Status |
|---|---|---|
| Sleep when off | Powers down and wakes on the switch | Planned |
| Low-voltage protection | Steps down, then off, on a low cell | Planned |
| Thermal regulation | Steps down when hot | Planned |

Until those are in, this isn't firmware to carry. See the [roadmap](roadmap.md).
