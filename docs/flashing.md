# Flashing

`cargo xtask` has a UPDI programmer built in, the same one as [wurkkos-ts21](https://github.com/sjparkinson/wurkkos-ts21). It runs on macOS and Linux, and sets up the serial port with `stty`.

## Only the plain adapter goes on this torch

Only a non-high-voltage UPDI adapter ever goes on the TS26's pads. Adafruit's High Voltage UPDI Friend never does, and that includes trying to recover a torch that's stopped talking. There's no exception for a torch that's already dead: if UPDI's gone, the torch is gone.

I lost the TS21 on 2026-11-06 after hand-holding the Friend on its pads. It then measured about 2.5 Ω between all three pads. I think 12 V or a slipped probe did it, and I can't prove either.

The Friend puts a 12 V pulse on the UPDI line on every falling edge of RTS, which probably means every time a tool opens the port, and nothing turns that off. The chip's RESET pin is rated for that, but nobody knows what else sits on these pads. Keep it for a breakout. [bench-plan.md](bench-plan.md#the-high-voltage-updi-friends-12-v-pulse) has the detail.

Both adapters are CH340s and show up as USB ID `1a86:7523`, with the same kind of port name, so nothing on the computer tells you which one you've plugged in. Look at the one in your hand, and keep the Friend unplugged and off the bench whenever the torch is open.

## What you need

A serial UPDI adapter and some way to hold it on three tiny pads.

The easy option is a ready-made USB UPDI programmer. [JLHawaii808's kit](https://jlhawaii808.com/products/updi-programmer-driver-reflashling-kit) is a CH340 serial UPDI adapter with USB-C, in 3.3 V and 4.5 V versions, and either works for Wurkkos lights (the 4.5 V one is listed for Sofirn, Wurkkos and Emisar). thefreeman sells one on BLF ([thread](https://budgetlightforum.com/t/wts-usb-updi-programmers-for-attiny1616-avr32dd20-drivers-sofirn-wurkkos-fireflylite-emisar/71459)) that comes with loose pogo-pin leads as well as the fixed probe.

The cheap option is a USB-serial adapter (CH340, CP2102, FT232) wired as a serial UPDI programmer, with TX and RX joined and that joined line going to the UPDI pad. [pymcuprog's README](https://github.com/microchip-pic-avr-tools/pymcuprog) suggests a series resistor of about 1 kΩ between TX and UPDI, and an adapter running at the same voltage as the MCU. The CH340 module in [pingle.org's guide](https://www.pingle.org/2022/08/07/flashing-a-3-pin-t1616-flashlight-with-new-firmware) worked wired straight through. Add pogo pins or a steady hand.

Watch the pad spacing before you rely on a fixed probe. gchart's standard three-pad layout is `+ - R` at 1.27 mm pitch, and that's what most probes expect. Some Wurkkos and Sofirn boards use 2.54 mm instead (thefreeman ships loose leads for exactly that reason), and some put the pads in a different order. The TS21's pads turned out to be about 1.75 mm apart, not the 1.27 mm I'd written down, and that's neither of those. So measure the pitch on your own board before trusting any probe. One that nearly fits sits on the edges of the pads, which is how it slips. Loose pogo leads work whatever the layout. Use a short USB cable too; JLHawaii808 notes failures with 1 m ones.

## The pads

Where the TS26's pads are, which is which and their pitch are in [hardware.md](hardware.md#the-flashing-pads), with how sure anyone is about each. Check there before you buy a fixed probe.

Whatever the layout, take the cell out, since the programmer powers the MCU through its supply pad (anything from 3.3 V to 5 V depending on the programmer, and the MCU's rated to 5.5 V). Before anything touches the UPDI pad, check with a multimeter that the pad you think is ground connects to the battery negative contact. Pad order isn't the same on every light, and a probe the wrong way round puts the supply on UPDI.

Measure the ohms between each pair of pads before the first contact and write them down, so you've got something to compare with if it stops talking. I don't know what a good TS26 reads. About 2.5 Ω between all three is what the dead TS21 read.

If you can manage it, hold the probe with something that can't slip rather than your hand.

## Find the port

Run `ls /dev/cu.*` before and after plugging the adapter in. On macOS it's something like `/dev/cu.usbserial-1410` or `/dev/cu.wchusbserial1410`. On Linux it's usually `/dev/ttyUSB0`, and you may need to be in the `dialout` group.

## Check it's talking

```sh
cargo xtask ping /dev/cu.usbserial-XXXX
```

This reads the chip's System Information Block and changes nothing, not even a reset. A real TS21 says `found tinyAVR P:0D:0-3`, so expect something like that. If you don't, see [If it doesn't talk](#if-it-doesnt-talk).

## Back up the factory firmware first

Do this before you flash anything. It's the only copy of Wurkkos's build and its calibration that you'll have.

```sh
cargo xtask backup /dev/cu.usbserial-XXXX
```

That puts the chip in programming mode, checks it's an ATtiny1616, and saves `backup/flash.hex` (all 16 KB), `backup/eeprom.hex` and `backup/fuses.hex`. The fuses are there for reference only. It refuses to run if `backup/flash.hex` already exists, so a second run can't overwrite your factory copy with this firmware.

`backup/` is git-ignored. Copy it somewhere safe too.

## The first flash is the `first-light` tag

The first thing that goes on a torch is the `first-light` tag and nothing else. It's a 628-byte image that clicks on and off at one dim level. It has no watchdog, no sleep and no interrupts, so there's very little in it to hang, and a light that works shows the pads, the programmer, the boost enable and the PWM all do what this repo thinks they do.

```sh
git switch --detach first-light
cargo xtask flash /dev/cu.usbserial-XXXX
git switch -
```

The full firmware only goes on once that's proven on the light, which is stage 3 of [bench-plan.md](bench-plan.md#stage-3-first-light-on-the-light). The TS21 went dark straight after a flash that verified, of a firmware with far more going on at start-up, and a firmware hang wasn't ruled out. Starting small means a dark torch here has a short list of causes.

`first-light` has no low-voltage protection and doesn't sleep, so don't leave a cell in it.

## Flash this firmware

```sh
cargo xtask flash /dev/cu.usbserial-XXXX
```

That builds the firmware, refuses it if the simulator finds a mistake, and then

1. puts the chip in programming mode and checks its signature (1E 94 21)
2. chip-erases it, which wipes flash and, unless the EESAVE fuse is set (Microchip ships it unset), EEPROM
3. writes the image a 64-byte page at a time, skipping blank pages (the erase already left them blank). The image is padded to all 16 KB with a CRC in the last two bytes, which the firmware checks at power-on, so the last page always gets written
4. reads all 16 KB back and compares
5. resets the chip and turns UPDI off, so the new firmware starts

If a write or the compare fails, step 5 still runs, so the chip isn't left in programming mode. Whatever was half-written then runs, but with the cell out it's powered by the programmer alone. Run the command again.

Step 1 is the only check on the chip before the erase, and the signature only says it's an ATtiny1616. A TS21 passes it, so does a chip whose fuses this firmware can't live with, and `flash` never looks at the fuses. Read `backup/fuses.hex` yourself first, and make sure you're in the right repo for the torch on the bench. `flash` with a file refuses a name without `ts26` or `0721` in it, but that's the name and nothing more. Step 4 only says the bytes arrived, and says nothing about whether they run.

A flash doesn't power-cycle the chip, so take the probe off with the cell out and leave it a few seconds, so VCC really collapses, before putting the cell in. Otherwise what the old firmware left in the reset flags could look like a hang.

If the light goes dark after a flash that verified, run `ping`. If it still talks, `cargo xtask peek /dev/cu.usbserial-XXXX` reads the running chip's registers and SRAM, which shows how far the firmware got. Then flash `first-light` again. A light that comes back points at the firmware, and one that doesn't points at the board. If it doesn't talk, see [below](#if-it-doesnt-talk), and stop there.

## If it's capped at level 50

The firmware quietly holds the light at level 50 if a check at power-on fails: flash doesn't match its CRC, the fuses aren't what it expects, or the watchdog reset the chip. It doesn't flash or tell you which, and it still works, so all you'll see is the ramp stopping low and turbo looking dim. None of the three has met a real TS26, so suspect the checks before the torch. In this order:

1. **A watchdog reset.** Take the cell out, wait a few seconds, and put it back. That's the only one this clears, so if the cap's gone, something bit. It might be a real hang, or the reset flags left over from before a flash, and if it comes back after use, it's the firmware or the watchdog's window, which nobody's seen on a real chip.
2. **The CRC.** A cap straight after a flash that verified points at the sum in `xtask/src/crc.rs`, which has never been checked by real hardware. After an interrupted flash, run `cargo xtask flash` again.
3. **The fuses.** Compare `backup/fuses.hex` with `selftest::Fuses::ok` in `src/selftest.rs`, which has never seen a real TS26's bytes. Nothing here writes fuses, and on this torch that's not something to start doing to clear a cap. A capped torch is a working torch.

The cap only lasts for the boot that saw the watchdog's flag, so a later reset of another kind clears it too.

## Going back

`restore` puts your backup's flash and EEPROM back. It writes every EEPROM page, blank ones included, since with EESAVE set the old contents survive the erase.

```sh
cargo xtask restore /dev/cu.usbserial-XXXX
```

For current Anduril, download a release zip from [ToyKeeper's releases](https://github.com/ToyKeeper/anduril/releases) and pull the TS26 build out of its `hex/` directory. Anduril's `MODELS` file lists model 0721 as `wurkkos-ts26`, and the hex is named after the release, so in `anduril.r2026-10-26.zip` it's `hex/anduril.r2026-10-26.wurkkos-ts26.hex`. Flash that.

```sh
cargo xtask flash /dev/cu.usbserial-XXXX path/to/anduril.r2026-10-26.wurkkos-ts26.hex
```

Unlike the TS21, which runs a Sofirn SP36 build, the TS26 has a build of its own, so there's no guessing which one to pick. I haven't flashed one onto a TS26 myself.

## If it doesn't talk

- "no reply from the chip" means the adapter heard itself and the chip said nothing, which usually means poor contact or the wrong pad on UPDI. It's also what you get with the probe in the air. Reseat the probe or check the pads again, and don't lean on it. The tool already retries once with a UPDI double break.
- "nothing echoed back" with the probe in the air means the adapter isn't joining TX and RX, so it isn't a serial UPDI adapter (or the joining wire's come off).
- "nothing echoed back" with the probe on the pads, from an adapter that says "no reply" in the air, is the bad one. Something's pulling the UPDI line down so hard the adapter can't hear its own bytes, which means the probe's on the ground pad or there's a short on the board. Better contact won't fix that. Take the probe off, check the pad order, and measure the ohms against the numbers you wrote down. That's a lesson from the dead TS21, and the Friend still stays off.
- If a DIY adapter never works with TX and RX tied directly, add the 1 kΩ resistor.
- thefreeman's older 3.3 V programmers had trouble with some lights, which is why the current one supplies 4.5 V. If yours is 3.3 V and flaky, that might be it.
- The anduril.click [UPDI guide](https://anduril.click/drivers/updi.html) and the [reflash reference](https://budgetlightforum.com/t/anduril-flashlight-reflash-reference-updated-for-2024/216942) cover more adapters and their quirks.

## How it works

The programmer writes flash and EEPROM, and only those. That's all a TS26 needs, and it keeps fuses out of reach, since a bad fuse write can turn off UPDI, and the only way back from that is 12 V on the pads, which this torch never gets.

`xtask/src/updi.rs` is the protocol, covering link set-up, the NVM programming key, reset, page-buffer writes through the NVM controller and reads in 256-byte blocks. The sequences follow Microchip's pymcuprog, and the registers come from the ATDF. `xtask/src/serial.rs` opens the port, configures it with `stty` (115200 baud, 8 data bits, even parity, 2 stop bits) and swallows the echo that a one-wire adapter sends back.

The tests run all of it against a pretend ATtiny1616 in `updi.rs`. That catches slips in framing and sequencing, but it was written from the same reading of the datasheet as the programmer, so the first run against a real chip is the real test. Start with `ping`, then `backup`.
