//! One pass of the firmware's loop, minus the hardware. A switch sample and
//! an ADC result in, a level and the next conversion out.

use crate::TICKS_PER_SECOND;
use crate::battery;
use crate::button::{Clicks, Event};
use crate::limits::{Health, Limits};
use crate::ramp::{self, Output};
use crate::selftest::Faults;
use crate::sensors::Convert;
use crate::temp::Calibration;
use crate::ui::{Aux, Mode, Ui};

/// How long the aux LEDs say why the light won't come on.
///
/// That's after a press, or after the light goes out. Three seconds, then
/// dark, so a flat cell with no protection of its own isn't drained by the
/// aux. Unmeasured.
pub const SHOW_TICKS: u16 = 3 * TICKS_PER_SECOND;

/// How long the aux shows bright red after a press that leaves it locked.
///
/// That's 3C locking it, or any other press while it's locked. Five seconds,
/// then dark, since steady bright red would flatten a full cell in about a
/// month. Unmeasured.
pub const LOCKED_TICKS: u16 = 5 * TICKS_PER_SECOND;

/// How long the torch rests between readings of the cell, five minutes.
///
/// After that many ticks of rest it takes one voltage reading, the way a
/// press does, and rests again. The 32 kHz oscillator that times it can be
/// 30 % out, so it's five minutes give or take a minute and a half.
/// Unmeasured.
pub const CHECK_TICKS: u16 = 5 * 60 * TICKS_PER_SECOND;

/// Under this many centivolts, read with the light off, off's dim blue goes
/// dark.
///
/// That's 3.30 V, where a rested Li-ion is close to empty. Unmeasured.
pub const AUX_DARK_BELOW: u16 = 330;

/// At or above this many centivolts, read with the light off, off's dim blue
/// comes back.
///
/// The 0.1 V over [`AUX_DARK_BELOW`] is the hysteresis, so a cell resting on
/// the line doesn't flick the aux on and off every check. Unmeasured.
pub const AUX_LIT_FROM: u16 = 340;

const _: () = assert!(AUX_DARK_BELOW < AUX_LIT_FROM);

#[derive(Clone, Debug)]
pub struct Torch {
    clicks: Clicks,
    ui: Ui,
    limits: Limits,
    /// Ticks left of [`SHOW_TICKS`] or [`LOCKED_TICKS`].
    showing: u16,
    /// Ticks of rest left until the next check of the cell, `0` meaning due.
    check_in: u16,
    /// Reading the cell with the switch up and the light off, either a
    /// check or a conversion that was still running when the torch went
    /// quiet. The torch doesn't rest until it's done.
    checking: bool,
    /// The last plausible reading with the light off was under
    /// [`AUX_DARK_BELOW`], and none since has been [`AUX_LIT_FROM`] or more.
    cell_low: bool,
}

impl Torch {
    /// `calibration` is the temperature sensor's, from the signature row, and
    /// `faults` what the power-on self-tests found. A fault shows on the aux
    /// from power-on.
    pub const fn new(calibration: Calibration, faults: Faults) -> Self {
        Self {
            clicks: Clicks::new(),
            ui: Ui::new(),
            limits: Limits::new(calibration, faults),
            showing: if faults.any() { SHOW_TICKS } else { 0 },
            check_in: 0,
            checking: false,
            cell_low: false,
        }
    }

    /// One 64 Hz tick, `pressed` meaning the switch is down and `adc` the
    /// ADC's result, if it has one ready. Returns the click or hold this
    /// sample finished, if any.
    ///
    /// With a flat cell or a fault, and the light out, clicks and holds never
    /// reach the UI. They show the aux colour instead, for [`SHOW_TICKS`]
    /// from the click or from the start of the hold, so a switch held down
    /// can't keep the aux lit. While a fault's fading the light out they
    /// still reach it, so 1C turns it off. A press reads the cell before its
    /// click counts, so a cell that's been charged is [`Health::Good`] by the
    /// time the click arrives, and that click turns the light on as usual.
    ///
    /// A click or the start of a hold that leaves the torch locked shows
    /// bright red for [`LOCKED_TICKS`], counted the same way.
    ///
    /// After [`CHECK_TICKS`] of rest, and on the first tick after power-on,
    /// it reads the cell, through [`Limits::tick`] with the light off just
    /// as a press does, and rests again once the reading's in. Any tick
    /// that isn't resting starts the count again, since the cell's read
    /// then anyway. Not with a fault, which nothing changes.
    pub fn tick(&mut self, pressed: bool, adc: Option<u16>) -> Option<Event> {
        self.showing = self.showing.saturating_sub(1);
        let event = self.clicks.tick_up_to(pressed, self.ui.most_clicks());
        if let Some(event) = event {
            let press = matches!(event, Event::Click(_) | Event::Hold { ticks: 0, .. });
            if self.limits.health() == Health::Good || self.level() > 0 {
                self.ui.handle(event);
                if press && self.ui.mode() == Mode::Lockout {
                    self.showing = LOCKED_TICKS;
                }
            } else if press {
                self.showing = SHOW_TICKS;
            }
        }
        let before = self.limits.health();
        let lit = self.level() > 0;
        // A conversion still running when the torch goes quiet, at the end
        // of a press with the light already off, finishes before it rests,
        // so every conversion started gets read.
        if !self.quiet() {
            self.checking = false;
        } else if self.limits.convert().is_some() || (self.check_in == 0 && before != Health::Fault)
        {
            self.checking = true;
        }
        if !self.resting() {
            let step = self.limits.tick(self.ui.output_level(), adc);
            if step.off {
                self.ui.off();
            }
            match step.unloaded {
                Some(volts) if !battery::plausible(volts) => {}
                Some(volts) if volts < AUX_DARK_BELOW => self.cell_low = true,
                Some(volts) if volts >= AUX_LIT_FROM => self.cell_low = false,
                _ => {}
            }
            if self.limits.convert().is_none() {
                self.checking = false;
            }
        }
        // Say why, from the moment it's decided, through any fade, and for a
        // while after the light's out. That while is counted from the first
        // dark tick, the one after the last it was lit, however it went out.
        let health = self.limits.health();
        if health != Health::Good && (before == Health::Good || lit) {
            self.showing = SHOW_TICKS;
        }
        // Checked again, so a pass that turns the light off has already
        // rested if the firmware powers down straight after it.
        if self.resting() {
            self.limits.rest();
            self.check_in = self.check_in.saturating_sub(1);
        } else {
            self.check_in = CHECK_TICKS;
        }
        event
    }

    /// Whether the switch and the light leave nothing to do, meaning the
    /// light's off and no click is being counted.
    const fn quiet(&self) -> bool {
        self.clicks.idle() && self.ui.output_level() == 0 && self.limits.level() == 0
    }

    /// Whether there's nothing to do until the switch moves or the next
    /// check of the cell, meaning it's [quiet](Self::quiet) and not reading
    /// the cell. The firmware powers down between these ticks, and a press
    /// ends it on the tick that sees the press, in time to read the cell
    /// before a click can turn the light on.
    pub const fn resting(&self) -> bool {
        self.quiet() && !self.checking
    }

    /// The level the LEDs should be at right now, `0` meaning off. That's what
    /// the UI wants, as far as the cell and the temperature allow.
    pub const fn level(&self) -> u8 {
        self.limits.level()
    }

    /// The conversion to start on the ADC after this tick, if any.
    pub const fn convert(&self) -> Option<Convert> {
        self.limits.convert()
    }

    pub fn output(&self) -> Output {
        ramp::output(self.level())
    }

    /// The UI's colour while all's well, but with lockout's red only for
    /// [`LOCKED_TICKS`] after a press, and blue only while the cell last read
    /// [`AUX_LIT_FROM`] or more with the light off, or never read under
    /// [`AUX_DARK_BELOW`]. Otherwise yellow for a flat cell and
    /// white for a fault, for [`SHOW_TICKS`] after a press or after the light
    /// goes out, and dark the rest of the time.
    pub const fn aux(&self) -> Aux {
        match self.limits.health() {
            Health::Good => match self.ui.aux() {
                Aux::Red if self.showing == 0 => Aux::Off,
                Aux::Blue if self.cell_low => Aux::Off,
                colour => colour,
            },
            _ if self.level() > 0 || self.showing == 0 => Aux::Off,
            Health::Flat => Aux::Yellow,
            Health::Fault => Aux::White,
        }
    }
}

/// The last value sent to the hardware, so the loop only rewrites the
/// timer and the enable pin when the output moves.
#[derive(Clone, Copy, Debug, Default)]
pub struct Changes<T> {
    last: Option<T>,
}

impl<T: Copy + PartialEq> Changes<T> {
    pub const fn new() -> Self {
        Self { last: None }
    }

    /// `Some(value)` the first time and whenever `value` differs from the
    /// last one seen, `None` otherwise.
    pub fn update(&mut self, value: T) -> Option<T> {
        if self.last == Some(value) {
            return None;
        }
        self.last = Some(value);
        Some(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::battery;
    use crate::button::{HOLD_TICKS, RELEASE_TICKS};
    use crate::ramp::CEILING;
    use crate::sensors::Channel;
    use crate::testing::{EXAMPLES, Rig, Rng, example, temperature_reading, voltage_reading};
    use crate::thermal;
    use crate::ui::{self, Mode, ON_LEVEL};

    fn dim() -> Output {
        ramp::output(ON_LEVEL)
    }

    fn run(torch: &mut Rig, pressed: bool, ticks: u16) {
        for _ in 0..ticks {
            torch.tick(pressed);
        }
    }

    fn clicks(torch: &mut Rig, n: u8) {
        for _ in 0..n {
            run(torch, true, 4);
            run(torch, false, 4);
        }
    }

    fn click(torch: &mut Rig) {
        clicks(torch, 1);
        run(torch, false, RELEASE_TICKS + 1);
    }

    /// On, and held at the top of the ramp.
    fn at_the_top(torch: &mut Rig) {
        click(torch);
        run(torch, true, 3 * TICKS_PER_SECOND);
        run(torch, false, RELEASE_TICKS + 1);
        assert_eq!(torch.level(), CEILING);
    }

    /// Every row's example, played the way `cargo xtask simulate` and the
    /// firmware's run in `cargo xtask hex` play it.
    #[test]
    fn every_example_holds() {
        for example in EXAMPLES {
            let level = Rig::play(example.actions()).level();
            assert!(
                example.expect.holds(level),
                "{}: expected {:?}, but it ends at level {level}",
                example.name,
                example.expect
            );
        }
    }

    #[test]
    fn starts_off() {
        let mut torch = Rig::default();
        assert_eq!(torch.tick(false), None);
        assert_eq!(torch.level(), 0);
        assert_eq!(torch.output(), Output::OFF);
    }

    #[test]
    fn a_click_turns_it_on_once_the_click_counts() {
        let mut torch = Rig::default();
        run(&mut torch, true, 4);
        run(&mut torch, false, RELEASE_TICKS);
        assert_eq!(torch.output(), Output::OFF);
        assert_eq!(torch.tick(false), Some(Event::Click(1)));
        assert_eq!(torch.output(), dim());
    }

    #[test]
    fn a_second_click_turns_it_off() {
        let mut torch = Rig::default();
        click(&mut torch);
        click(&mut torch);
        assert_eq!(torch.output(), Output::OFF);
    }

    #[test]
    fn lockout_never_lights() {
        let mut torch = Rig::default();
        clicks(&mut torch, 3);
        run(&mut torch, false, RELEASE_TICKS + 1);
        run(&mut torch, true, 2 * TICKS_PER_SECOND);
        assert_eq!(torch.level(), 0);
        run(&mut torch, false, RELEASE_TICKS + 1);
        click(&mut torch);
        assert_eq!(torch.level(), 0);
        assert_eq!(torch.aux(), Aux::Red);
    }

    /// Clicks `n` times and stops on the tick `event` arrives, whether that's
    /// as the last click is let go or once the switch has been left alone.
    fn tap(torch: &mut Rig, n: u8, event: Event) {
        for _ in 0..n {
            run(torch, true, 4);
            for _ in 0..4 {
                if torch.tick(false) == Some(event) {
                    return;
                }
            }
        }
        for _ in 0..TICKS_PER_SECOND {
            if torch.tick(false) == Some(event) {
                return;
            }
        }
        panic!("no {event:?}");
    }

    #[test]
    fn a_click_counts_as_it_is_let_go_when_no_more_would_mean_anything() {
        let mut torch = Rig::default();
        click(&mut torch);
        run(&mut torch, true, 4);
        assert_eq!(torch.tick(false), Some(Event::Click(1)), "off, from on");
        assert_eq!(torch.level(), 0);
        clicks(&mut torch, 2);
        run(&mut torch, true, 4);
        assert_eq!(torch.tick(false), Some(Event::Click(3)), "3C, from off");
        assert_eq!(torch.torch.ui.mode(), Mode::Lockout);
    }

    /// Locked, it still waits, so four presses in a pocket are a 4C, which
    /// does nothing, rather than 3C to unlock and 1C to light.
    #[test]
    fn four_clicks_in_lockout_neither_unlock_it_nor_light_it() {
        let mut torch = Rig::default();
        tap(&mut torch, 3, Event::Click(3));
        run(&mut torch, false, RELEASE_TICKS + 1);
        tap(&mut torch, 4, Event::Click(4));
        assert_eq!(torch.torch.ui.mode(), Mode::Lockout);
        run(&mut torch, false, TICKS_PER_SECOND);
        assert_eq!(torch.level(), 0);
    }

    #[test]
    fn lockout_shows_bright_red_for_five_seconds_after_each_press() {
        let mut torch = Rig::default();
        tap(&mut torch, 3, Event::Click(3));
        assert_eq!(torch.aux(), Aux::Red, "as 3C locks it");
        for tick in 1..LOCKED_TICKS {
            torch.tick(false);
            assert_eq!(torch.aux(), Aux::Red, "tick {tick}");
        }
        torch.tick(false);
        assert_eq!(torch.aux(), Aux::Off);
        tap(&mut torch, 1, Event::Click(1));
        assert_eq!(torch.aux(), Aux::Red, "a press that does nothing");
        run(&mut torch, false, LOCKED_TICKS);
        assert_eq!(torch.aux(), Aux::Off);
        tap(&mut torch, 3, Event::Click(3));
        assert_eq!(torch.aux(), Aux::Blue, "3C unlocks it");
    }

    #[test]
    fn a_hold_in_lockout_cannot_keep_the_red_lit() {
        let mut torch = Rig::default();
        tap(&mut torch, 3, Event::Click(3));
        run(&mut torch, false, LOCKED_TICKS);
        assert_eq!(torch.aux(), Aux::Off);
        run(&mut torch, true, HOLD_TICKS + 1);
        assert_eq!(torch.aux(), Aux::Red, "from the start of the hold");
        run(&mut torch, true, LOCKED_TICKS);
        assert_eq!(torch.aux(), Aux::Off, "still held");
    }

    /// Ticks with the switch up until a check of the cell has started and
    /// finished, and the torch rests again. Returns how many that took.
    fn next_check(torch: &mut Rig) -> u32 {
        let mut ticks = 0;
        while torch.resting() {
            torch.tick(false);
            ticks += 1;
            assert!(ticks <= u32::from(CHECK_TICKS) + 1, "no check");
        }
        while !torch.resting() {
            torch.tick(false);
            ticks += 1;
        }
        ticks
    }

    /// One check, from the first tick of rest, the way every check after
    /// power-on goes. Two ticks awake and the third resting again.
    fn assert_a_check(torch: &mut Rig, why: &str) {
        torch.tick(false);
        assert!(!torch.resting(), "{why}: the check's first tick");
        assert_eq!(
            torch.convert(),
            Some(Convert {
                channel: Channel::Voltage,
                setup: true
            }),
            "{why}"
        );
        torch.tick(false);
        assert!(!torch.resting(), "{why}: the check's second tick");
        assert!(torch.convert().is_some(), "{why}");
        torch.tick(false);
        assert!(torch.resting(), "{why}: the reading's in");
        assert_eq!(torch.convert(), None, "{why}");
    }

    /// Ticks with the switch up, every one of them resting.
    fn rest(torch: &mut Rig, ticks: u16, why: &str) {
        for tick in 0..ticks {
            torch.tick(false);
            assert!(torch.resting(), "{why}: tick {tick} of rest");
        }
    }

    #[test]
    fn the_cell_is_read_on_the_first_tick_and_after_every_five_minutes_of_rest() {
        assert_eq!(CHECK_TICKS, 5 * 60 * 64);
        let mut torch = Rig::default();
        assert!(torch.resting(), "before the first tick");
        assert_a_check(&mut torch, "power-on");
        for why in ["the first after", "the second", "the third"] {
            // The tick the reading came in was the first of the rest.
            rest(&mut torch, CHECK_TICKS - 1, why);
            assert_a_check(&mut torch, why);
        }
    }

    #[test]
    fn a_press_or_the_light_starts_the_five_minutes_again() {
        let mut torch = Rig::default();
        run(&mut torch, false, 3);
        rest(&mut torch, CHECK_TICKS / 2, "half way");
        // 2C does nothing from off, but the switch reads the cell.
        tap(&mut torch, 2, Event::Click(2));
        while !torch.resting() {
            torch.tick(false);
        }
        rest(&mut torch, CHECK_TICKS - 1, "after the press");
        assert_a_check(&mut torch, "five minutes after the press");
        rest(&mut torch, CHECK_TICKS / 2, "half way");
        click(&mut torch);
        run(&mut torch, false, 10 * TICKS_PER_SECOND);
        tap(&mut torch, 1, Event::Click(1));
        assert_eq!(torch.level(), 0);
        assert!(torch.resting(), "out as 1C's let go");
        rest(&mut torch, CHECK_TICKS - 1, "after the light");
        assert_a_check(&mut torch, "five minutes after the light");
    }

    #[test]
    fn blue_stays_lit_through_three_days_of_rest_with_a_good_cell() {
        let mut torch = Rig::default();
        let (mut checks, mut resting) = (0, true);
        for tick in 0..3 * 24 * 60 * 60 * u32::from(TICKS_PER_SECOND) {
            torch.tick(false);
            assert_eq!(torch.aux(), Aux::Blue, "tick {tick}");
            assert_eq!(torch.level(), 0, "tick {tick}");
            checks += u32::from(resting && !torch.resting());
            resting = torch.resting();
        }
        // One at power-on and one every five minutes and two ticks after.
        assert_eq!(checks, 864);
    }

    #[test]
    fn a_low_cell_at_power_on_darkens_blue_from_the_first_check() {
        let mut torch = Rig::default();
        torch.adc.voltage = voltage_reading(320);
        run(&mut torch, false, 2);
        assert_eq!(torch.aux(), Aux::Blue, "before the reading's in");
        torch.tick(false);
        assert_eq!(torch.aux(), Aux::Off, "the third tick, as it comes in");
        for _ in 0..3 {
            next_check(&mut torch);
            assert_eq!(torch.aux(), Aux::Off);
        }
        // It's only the aux. The light still comes on.
        click(&mut torch);
        assert_eq!(torch.level(), ON_LEVEL);
        click(&mut torch);
        assert_eq!(torch.aux(), Aux::Off);
    }

    #[test]
    fn blue_goes_dark_under_3_30_volts_and_comes_back_at_3_40() {
        for cv in [329, 330, 335, 339, 340] {
            assert_eq!(battery::centivolts(voltage_reading(cv)), cv);
        }
        let mut torch = Rig::default();
        for (cv, aux, why) in [
            (330, Aux::Blue, "3.30 V is still lit"),
            (329, Aux::Off, "under 3.30 V"),
            (335, Aux::Off, "3.35 V doesn't light it"),
            (339, Aux::Off, "nor 3.39 V"),
            (340, Aux::Blue, "3.40 V does"),
            (335, Aux::Blue, "3.35 V doesn't darken it"),
            (329, Aux::Off, "under 3.30 V again"),
        ] {
            torch.adc.voltage = voltage_reading(cv);
            next_check(&mut torch);
            assert_eq!(torch.aux(), aux, "{why}");
            // And only at a check.
            torch.adc.voltage = voltage_reading(if aux == Aux::Off { 420 } else { 300 });
            rest(&mut torch, CHECK_TICKS - 1, why);
            assert_eq!(torch.aux(), aux, "{why}, till the next check");
            torch.adc.voltage = voltage_reading(cv);
        }
    }

    #[test]
    fn charging_a_low_cell_at_rest_brings_blue_back_at_the_next_check() {
        let mut torch = Rig::default();
        torch.adc.voltage = voltage_reading(320);
        next_check(&mut torch);
        assert_eq!(torch.aux(), Aux::Off);
        torch.adc.voltage = voltage_reading(380);
        let ticks = next_check(&mut torch);
        assert!(ticks <= u32::from(CHECK_TICKS) + 3, "{ticks}");
        assert_eq!(torch.aux(), Aux::Blue);
    }

    #[test]
    fn a_press_reads_the_cell_for_the_aux_without_waiting_for_a_check() {
        let mut torch = Rig::default();
        run(&mut torch, false, TICKS_PER_SECOND);
        assert_eq!(torch.aux(), Aux::Blue);
        torch.adc.voltage = voltage_reading(320);
        // Down for long enough for one reading, which is a click's worth.
        run(&mut torch, true, 4);
        assert_eq!(torch.aux(), Aux::Off, "pressed");
        // Made a 2C, which does nothing.
        run(&mut torch, false, 4);
        tap(&mut torch, 1, Event::Click(2));
        assert_eq!(torch.level(), 0);
        torch.adc.voltage = voltage_reading(380);
        tap(&mut torch, 2, Event::Click(2));
        assert_eq!(torch.aux(), Aux::Blue, "a press that does nothing");
    }

    #[test]
    fn readings_with_the_light_on_never_move_the_aux() {
        // A good cell at the press, then low while it's lit, and blue's
        // still there when it goes out.
        let mut torch = Rig::default();
        click(&mut torch);
        torch.adc.voltage = voltage_reading(310);
        run(&mut torch, false, 60 * TICKS_PER_SECOND);
        assert_eq!(torch.level(), ON_LEVEL, "over 3.00 V, so no step down");
        tap(&mut torch, 1, Event::Click(1));
        assert_eq!(torch.aux(), Aux::Blue, "as it goes out");
        rest(&mut torch, CHECK_TICKS - 1, "after the light");
        assert_eq!(torch.aux(), Aux::Blue, "till the next check");
        assert_a_check(&mut torch, "five minutes on");
        assert_eq!(torch.aux(), Aux::Off, "which reads it");
        // And the other way, a low cell at rest that reads well lit.
        click(&mut torch);
        torch.adc.voltage = voltage_reading(400);
        run(&mut torch, false, 60 * TICKS_PER_SECOND);
        tap(&mut torch, 1, Event::Click(1));
        assert_eq!(torch.aux(), Aux::Off, "as it goes out");
        next_check(&mut torch);
        assert_eq!(torch.aux(), Aux::Blue, "at the next check");
    }

    #[test]
    fn a_check_never_lights_it_or_changes_the_mode() {
        let mut torch = Rig::default();
        for _ in 0..3 * u32::from(CHECK_TICKS) {
            torch.tick(false);
            assert_eq!(torch.level(), 0);
            assert_eq!(torch.torch.ui.mode(), Mode::Off);
            assert_eq!(torch.torch.ui.output_level(), 0);
        }
        tap(&mut torch, 3, Event::Click(3));
        run(&mut torch, false, LOCKED_TICKS);
        assert_eq!(torch.aux(), Aux::Off, "red for five seconds only");
        torch.adc.voltage = voltage_reading(320);
        for _ in 0..3 {
            next_check(&mut torch);
            assert_eq!(torch.torch.ui.mode(), Mode::Lockout);
            assert_eq!(torch.level(), 0);
            assert_eq!(torch.aux(), Aux::Off);
        }
        // A low cell doesn't touch the red.
        tap(&mut torch, 1, Event::Click(1));
        for tick in 1..LOCKED_TICKS {
            torch.tick(false);
            assert_eq!(torch.aux(), Aux::Red, "tick {tick}");
        }
        torch.tick(false);
        assert_eq!(torch.aux(), Aux::Off);
        // Unlocked, it's off with blue dark for the low cell.
        tap(&mut torch, 3, Event::Click(3));
        run(&mut torch, false, RELEASE_TICKS + 1);
        assert_eq!(torch.torch.ui.mode(), Mode::Off);
        assert_eq!(torch.aux(), Aux::Off);
    }

    /// A reading at rest that can't be real leaves the aux as it was, and
    /// since each rest tick forgives the glitches before it, no run of them
    /// at rest adds up to a fault. The next press reads the cell four times
    /// a second, and four of them in a row there still are one.
    #[test]
    fn garbage_at_rest_leaves_the_aux_alone_and_is_no_fault() {
        for garbage in [0, voltage_reading(60), 4 * 1023] {
            let mut torch = Rig::default();
            torch.adc.voltage = voltage_reading(320);
            next_check(&mut torch);
            assert_eq!(torch.aux(), Aux::Off, "{garbage}");
            torch.adc.voltage = garbage;
            for _ in 0..12 {
                next_check(&mut torch);
                assert_eq!(torch.aux(), Aux::Off, "{garbage}");
                assert_eq!(torch.torch.limits.health(), Health::Good, "{garbage}");
            }
            torch.adc.voltage = voltage_reading(380);
            next_check(&mut torch);
            assert_eq!(torch.aux(), Aux::Blue, "{garbage}");
            torch.adc.voltage = garbage;
            for _ in 0..12 {
                next_check(&mut torch);
                assert_eq!(torch.aux(), Aux::Blue, "{garbage}");
                assert_eq!(torch.torch.limits.health(), Health::Good, "{garbage}");
            }
            // A good cell at the press, and it lights as usual.
            torch.adc.voltage = voltage_reading(380);
            click(&mut torch);
            assert_eq!(torch.level(), ON_LEVEL, "{garbage}");
            click(&mut torch);
            // A second's hold is four readings, which is a fault.
            torch.adc.voltage = garbage;
            run(&mut torch, true, 2 * TICKS_PER_SECOND);
            assert_eq!(torch.torch.limits.health(), Health::Fault, "{garbage}");
        }
    }

    /// The examples about the cell at rest only say how the light ends up.
    /// This holds them to the aux their comments describe as well.
    #[test]
    fn the_rest_examples_end_on_the_aux_they_describe() {
        let rows = [
            ("Flat cell, charged, left alone", Aux::Blue),
            ("Low cell, at rest", Aux::Off),
            ("Low cell, read by a press", Aux::Off),
            ("Low cell, charged at rest", Aux::Blue),
            ("A cell it can't read, at rest", Aux::Off),
        ];
        for (row, aux) in rows {
            let rig = Rig::play(example(row).actions());
            assert_eq!(rig.aux(), aux, "{row}");
            assert_eq!(rig.torch.limits.health(), Health::Good, "{row}");
        }
        // "Low cell, at rest" ends lit, so its dark blue is before the 1C,
        // and after it's clicked off again.
        let mut low = Rig::play(example("Low cell, at rest").actions().take(2));
        assert_eq!(low.aux(), Aux::Off, "dark before the 1C");
        low.adc.voltage = voltage_reading(320);
        click(&mut low);
        assert_eq!(low.level(), ON_LEVEL);
        click(&mut low);
        assert_eq!(low.aux(), Aux::Off, "and once it's off again");
        // "A cell it can't read, at rest" ends lit too, so the blue it
        // keeps through the garbage is before the 1C.
        let garbage = Rig::play(example("A cell it can't read, at rest").actions().take(2));
        assert_eq!(garbage.aux(), Aux::Blue, "blue through the garbage");
        assert_eq!(garbage.torch.limits.health(), Health::Good, "no fault yet");
    }

    #[test]
    fn a_check_whose_result_never_comes_still_ends() {
        let mut torch = Rig::default();
        run(&mut torch, false, 3);
        torch.adc.voltage = voltage_reading(320);
        next_check(&mut torch);
        assert_eq!(torch.aux(), Aux::Off);
        rest(&mut torch, CHECK_TICKS - 1, "to the next check");
        // No result at all, which reads as zero.
        for _ in 0..2 {
            torch.torch.tick(false, None);
            assert!(!torch.resting());
        }
        torch.torch.tick(false, None);
        assert!(torch.resting());
        assert_eq!(torch.convert(), None);
        assert_eq!(torch.aux(), Aux::Off);
        assert_eq!(torch.torch.limits.health(), Health::Good);
    }

    #[test]
    fn a_flat_cell_charged_at_rest_is_good_again_at_the_next_check() {
        let mut torch = Rig::default();
        click(&mut torch);
        torch.adc.voltage = voltage_reading(280);
        run(&mut torch, false, 10 * TICKS_PER_SECOND);
        assert_eq!(torch.torch.limits.health(), Health::Flat);
        // Rested, but not charged.
        torch.adc.voltage = voltage_reading(350);
        for _ in 0..3 {
            next_check(&mut torch);
            assert_eq!(torch.torch.limits.health(), Health::Flat);
            assert_eq!(torch.aux(), Aux::Off, "yellow only after a press");
        }
        torch.adc.voltage = voltage_reading(380);
        let ticks = next_check(&mut torch);
        assert!(ticks <= u32::from(CHECK_TICKS) + 3, "{ticks}");
        assert_eq!(torch.torch.limits.health(), Health::Good, "with no press");
        assert_eq!(torch.aux(), Aux::Blue);
        assert_eq!(torch.level(), 0, "and still off");
        click(&mut torch);
        assert_eq!(torch.level(), ON_LEVEL);
    }

    #[test]
    fn a_fault_skips_the_checks() {
        let mut torch = Rig::new(Faults {
            hang: true,
            ..Faults::NONE
        });
        for tick in 0..2 * u32::from(CHECK_TICKS) {
            torch.tick(false);
            assert!(torch.resting(), "tick {tick}");
            assert_eq!(torch.convert(), None, "tick {tick}");
        }
        // And one that comes up later, from a cell it can't read.
        let mut torch = Rig::default();
        click(&mut torch);
        torch.adc.voltage = 0;
        run(&mut torch, false, 5 * TICKS_PER_SECOND);
        assert_eq!(torch.torch.limits.health(), Health::Fault);
        assert!(torch.resting());
        for tick in 0..2 * u32::from(CHECK_TICKS) {
            torch.tick(false);
            assert!(torch.resting(), "tick {tick}");
        }
    }

    /// A hold from off goes quiet the tick it's let go. A conversion that's
    /// still running then gets read before the torch rests, so the cell
    /// changing on the hold's last tick shows on the aux exactly when a
    /// conversion was left running. Every one of a round's 16 ticks is the
    /// last for one of these holds.
    #[test]
    fn a_conversion_running_as_a_hold_ends_is_read_before_it_rests() {
        let (mut read, mut not) = (0, 0);
        for down in HOLD_TICKS + 1..HOLD_TICKS + 17 {
            let mut torch = Rig::default();
            run(&mut torch, false, 3);
            torch.adc.voltage = voltage_reading(320);
            run(&mut torch, true, down - 1);
            assert_eq!(torch.aux(), Aux::Off, "{down}");
            torch.adc.voltage = voltage_reading(380);
            torch.tick(true);
            let running = torch.convert().is_some();
            assert_eq!(torch.tick(false), Some(Event::Release { clicks: 0 }));
            run(&mut torch, false, 2);
            assert!(torch.resting(), "{down}");
            let aux = if running { Aux::Blue } else { Aux::Off };
            assert_eq!(torch.aux(), aux, "{down}");
            read += u32::from(running);
            not += u32::from(!running);
        }
        assert!(read > 0 && not > 0, "{read} read, {not} not");
    }

    #[test]
    fn the_ramp_never_asks_for_more_than_the_ceiling() {
        let mut torch = Rig::default();
        click(&mut torch);
        run(&mut torch, true, 10 * TICKS_PER_SECOND);
        assert_eq!(torch.level(), CEILING);
        assert_eq!(torch.output(), ramp::output(ramp::CEILING));
    }

    #[test]
    fn rests_only_with_the_light_off_and_no_click_counting() {
        let mut torch = Rig::default();
        assert!(torch.resting());
        run(&mut torch, true, 4);
        assert!(!torch.resting(), "pressed");
        run(&mut torch, false, RELEASE_TICKS);
        assert!(!torch.resting(), "a click waiting to count");
        run(&mut torch, false, 1);
        assert!(!torch.resting(), "on");
        click(&mut torch);
        assert!(torch.resting());
    }

    #[test]
    fn a_locked_torch_rests() {
        let mut torch = Rig::default();
        clicks(&mut torch, 3);
        run(&mut torch, false, RELEASE_TICKS + 1);
        assert!(torch.resting());
    }

    #[test]
    fn the_aux_leds_follow_the_ui() {
        let mut torch = Rig::default();
        assert_eq!(torch.aux(), Aux::Blue);
        click(&mut torch);
        assert_eq!(torch.aux(), Aux::Off);
    }

    #[test]
    fn a_flat_cell_turns_off_shows_yellow_and_ignores_clicks_until_charged() {
        let mut torch = Rig::default();
        click(&mut torch);
        torch.adc.voltage = voltage_reading(280);
        run(&mut torch, false, 10 * TICKS_PER_SECOND);
        assert_eq!(torch.level(), 0);
        assert_eq!(torch.ui.output_level(), 0, "the UI's off too");
        // Not left with the UI still on and the light out.
        assert!(torch.resting());
        // Yellow for a while after it went out, then dark.
        run(&mut torch, false, SHOW_TICKS);
        assert_eq!(torch.aux(), Aux::Off);
        // A click shows yellow again, and lights nothing.
        click(&mut torch);
        assert_eq!(torch.level(), 0);
        assert_eq!(torch.aux(), Aux::Yellow);
        run(&mut torch, false, SHOW_TICKS);
        assert_eq!(torch.aux(), Aux::Off);
        // Nor does a hold, or 3C.
        run(&mut torch, true, TICKS_PER_SECOND);
        run(&mut torch, false, RELEASE_TICKS + 1);
        clicks(&mut torch, 3);
        run(&mut torch, false, RELEASE_TICKS + 1);
        assert_eq!(torch.level(), 0);
        assert_eq!(torch.aux(), Aux::Yellow, "not locked out");
        // Charged, and the next click turns it on as if new.
        torch.adc.voltage = voltage_reading(380);
        click(&mut torch);
        assert_eq!(torch.level(), ON_LEVEL);
        click(&mut torch);
        assert_eq!(torch.aux(), Aux::Blue);
    }

    #[test]
    fn going_flat_shows_yellow_for_the_three_seconds_after() {
        let mut torch = Rig::default();
        click(&mut torch);
        torch.adc.voltage = voltage_reading(280);
        while torch.level() != 0 {
            torch.tick(false);
        }
        for _ in 0..SHOW_TICKS {
            assert_eq!(torch.aux(), Aux::Yellow);
            torch.tick(false);
        }
        assert_eq!(torch.aux(), Aux::Off);
    }

    #[test]
    fn the_pass_that_turns_the_light_off_has_already_rested() {
        let mut torch = Rig::default();
        click(&mut torch);
        torch.adc.voltage = voltage_reading(280);
        while torch.level() != 0 {
            torch.tick(false);
        }
        assert!(torch.resting());
        assert_eq!(
            torch.convert(),
            None,
            "nothing's left to read in power-down"
        );
    }

    #[test]
    fn a_cell_it_cant_read_fades_it_out_and_shows_white() {
        let mut torch = Rig::default();
        at_the_top(&mut torch);
        torch.adc.voltage = 0;
        run(&mut torch, false, TICKS_PER_SECOND);
        // The fault's in by now, and the light's on its way down.
        assert!(torch.level() > 0 && torch.level() < CEILING);
        assert_eq!(torch.aux(), Aux::Off, "out while the main LED's lit");
        run(&mut torch, false, 2 * TICKS_PER_SECOND);
        assert_eq!(torch.level(), 0);
        assert_eq!(torch.aux(), Aux::White);
        run(&mut torch, false, SHOW_TICKS);
        assert_eq!(torch.aux(), Aux::Off);
        // A good reading changes nothing. Only a reset does.
        torch.adc.voltage = voltage_reading(380);
        click(&mut torch);
        assert_eq!(torch.level(), 0);
        assert_eq!(torch.aux(), Aux::White);
    }

    #[test]
    fn a_click_still_turns_off_a_fault_thats_fading() {
        let mut torch = Rig::default();
        at_the_top(&mut torch);
        torch.adc.voltage = 0;
        while torch.torch.limits.health() == Health::Good {
            torch.tick(false);
        }
        run(&mut torch, true, 4);
        assert!(torch.level() > 0, "still fading when the click counts");
        assert_eq!(torch.tick(false), Some(Event::Click(1)), "as it's let go");
        assert_eq!(torch.level(), 0);
    }

    /// `testing::EXAMPLES` says three seconds, so this counts them in ticks of the
    /// 64 Hz clock rather than in [`SHOW_TICKS`].
    #[test]
    fn the_aux_says_why_for_three_seconds_to_the_tick() {
        const THREE_SECONDS: u16 = 3 * 64;
        assert_eq!(TICKS_PER_SECOND, 64);
        // From power-on, with a failed check.
        let mut torch = Rig::new(Faults {
            hang: true,
            ..Faults::NONE
        });
        for tick in 1..THREE_SECONDS {
            torch.tick(false);
            assert_eq!(torch.aux(), Aux::White, "tick {tick}");
        }
        torch.tick(false);
        assert_eq!(torch.aux(), Aux::Off);
        // From a click on a flat cell, counted from the tick the click
        // counts.
        let mut torch = Rig::default();
        click(&mut torch);
        torch.adc.voltage = voltage_reading(280);
        run(&mut torch, false, 20 * TICKS_PER_SECOND);
        assert_eq!(torch.aux(), Aux::Off);
        run(&mut torch, true, 4);
        run(&mut torch, false, RELEASE_TICKS);
        assert_eq!(torch.aux(), Aux::Off, "not before the click counts");
        assert_eq!(torch.tick(false), Some(Event::Click(1)));
        assert_eq!(torch.aux(), Aux::Yellow);
        for tick in 1..THREE_SECONDS {
            torch.tick(false);
            assert_eq!(torch.aux(), Aux::Yellow, "tick {tick}");
        }
        torch.tick(false);
        assert_eq!(torch.aux(), Aux::Off);
    }

    /// The three seconds start again on every tick of the fade, so they run
    /// from the first tick the light's out and not from the tick the fault
    /// was decided, which is most of two seconds earlier from the top.
    #[test]
    fn a_fault_that_fades_out_shows_white_for_the_three_seconds_after() {
        let mut torch = Rig::default();
        at_the_top(&mut torch);
        torch.adc.voltage = 0;
        while torch.torch.limits.health() == Health::Good {
            torch.tick(false);
        }
        let mut fade = 0;
        while torch.level() != 0 {
            assert_eq!(torch.aux(), Aux::Off, "out while the main LED's lit");
            torch.tick(false);
            fade += 1;
        }
        assert!(fade > TICKS_PER_SECOND, "a fade of {fade} ticks");
        // That tick was the first with the light out, which is where the
        // three seconds count from.
        for tick in 1..SHOW_TICKS {
            assert_eq!(torch.aux(), Aux::White, "tick {tick}");
            torch.tick(false);
        }
        assert_eq!(torch.aux(), Aux::White);
        torch.tick(false);
        assert_eq!(torch.aux(), Aux::Off);
    }

    #[test]
    fn a_fault_clicked_off_mid_fade_shows_white_for_the_three_seconds_after() {
        let mut torch = Rig::default();
        at_the_top(&mut torch);
        torch.adc.voltage = 0;
        while torch.torch.limits.health() == Health::Good {
            torch.tick(false);
        }
        run(&mut torch, false, TICKS_PER_SECOND);
        run(&mut torch, true, 4);
        assert!(torch.level() > 0, "still fading when the click counts");
        assert_eq!(torch.tick(false), Some(Event::Click(1)));
        assert_eq!(torch.level(), 0);
        for tick in 1..SHOW_TICKS {
            assert_eq!(torch.aux(), Aux::White, "tick {tick}");
            torch.tick(false);
        }
        assert_eq!(torch.aux(), Aux::White);
        torch.tick(false);
        assert_eq!(torch.aux(), Aux::Off);
    }

    #[test]
    fn heat_lowers_the_light_and_leaves_the_ui_where_it_was() {
        let mut torch = Rig::default();
        at_the_top(&mut torch);
        torch.adc.temperature = temperature_reading(thermal::celsius(80));
        run(&mut torch, false, 120 * TICKS_PER_SECOND);
        assert_eq!(torch.level(), thermal::FLOOR);
        assert_eq!(torch.output(), ramp::output(torch.level()));
        assert_eq!(torch.ui.output_level(), CEILING);
    }

    /// A switch that's stuck down from the moment the battery goes in, say a
    /// jammed button. It's a hold from off, which does nothing.
    #[test]
    fn a_switch_held_from_power_on_stays_off() {
        let mut torch = Rig::default();
        for _ in 0..10 * 60 * u32::from(TICKS_PER_SECOND) {
            torch.tick(true);
            assert_eq!(torch.level(), 0);
        }
    }

    /// Heat doesn't care whether the switch is down. The light follows the
    /// thermal ceiling while the ramp's still being held.
    #[test]
    fn a_switch_held_while_on_still_answers_to_heat() {
        let mut torch = Rig::default();
        click(&mut torch);
        run(&mut torch, true, 10 * TICKS_PER_SECOND);
        assert_eq!(torch.level(), CEILING);
        torch.adc.temperature = temperature_reading(thermal::celsius(80));
        assert!(thermal::celsius(80) > thermal::LIMIT);
        for _ in 0..120 * TICKS_PER_SECOND {
            torch.tick(true);
        }
        assert_eq!(torch.level(), thermal::FLOOR);
        assert_eq!(torch.ui.output_level(), CEILING, "still held at the top");
    }

    #[test]
    fn a_fault_never_lights_and_shows_white_after_power_on_and_a_press() {
        for faults in [
            Faults {
                flash: true,
                ..Faults::NONE
            },
            Faults {
                fuses: true,
                ..Faults::NONE
            },
            Faults {
                hang: true,
                ..Faults::NONE
            },
        ] {
            let mut torch = Rig::new(faults);
            assert_eq!(torch.aux(), Aux::White, "{faults:?}");
            run(&mut torch, false, SHOW_TICKS);
            assert_eq!(torch.aux(), Aux::Off, "{faults:?}");
            click(&mut torch);
            assert_eq!(torch.level(), 0, "{faults:?}");
            assert_eq!(torch.aux(), Aux::White, "{faults:?}");
            run(&mut torch, false, SHOW_TICKS);
            // A hold shows it from its start, and no longer, however long
            // the switch stays down.
            run(&mut torch, true, 30 * TICKS_PER_SECOND);
            assert_eq!(torch.level(), 0, "{faults:?}");
            assert_eq!(torch.aux(), Aux::Off, "{faults:?}");
        }
    }

    #[test]
    fn a_resting_torch_reads_nothing_and_a_press_starts_the_adc() {
        let mut torch = Rig::default();
        run(&mut torch, false, 64);
        assert!(torch.resting());
        assert_eq!(torch.convert(), None);
        torch.tick(true);
        assert!(!torch.resting());
        assert!(torch.convert().is_some_and(|convert| convert.setup));
    }

    #[test]
    fn still_turns_off_after_hours_on() {
        let mut torch = Rig::default();
        click(&mut torch);
        // Past where button::Clicks' tick count saturates.
        for _ in 0..4 {
            run(&mut torch, false, u16::MAX);
        }
        assert_eq!(torch.output(), dim());
        click(&mut torch);
        assert_eq!(torch.output(), Output::OFF);
    }

    /// A number from `low` to `high`, both included.
    fn between(rng: &mut Rng, low: u16, high: u16) -> u16 {
        low + rng.next_u16() % (high - low + 1)
    }

    /// How the noise passes the time.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Pace {
        /// Rests and changes of cell of half a minute at most, so the
        /// switching's thick and the cell's rarely checked at rest.
        Quick,
        /// The same switching, but a quarter of the rests and half the
        /// changes of cell last minutes, up to three checks' worth.
        Slow,
    }

    /// How long the switch stays down, or up, this time. Short enough for a
    /// click, either side of the tick a press becomes a hold or a click
    /// counts, or long, so that 1C, 3C, 1H, 2H and everything that isn't one
    /// of them all turn up often. At [`Pace::Slow`], some rests are long
    /// enough for checks of the cell.
    fn span(rng: &mut Rng, pressed: bool, pace: Pace) -> u16 {
        let roll = rng.next_u16() % 16;
        if pressed {
            match roll {
                0..=8 => between(rng, 1, 8),
                9..=10 => between(rng, HOLD_TICKS - 3, HOLD_TICKS + 3),
                _ => between(rng, HOLD_TICKS, 4 * TICKS_PER_SECOND),
            }
        } else {
            match roll {
                12..=15 if pace == Pace::Slow => between(rng, CHECK_TICKS / 4, 3 * CHECK_TICKS),
                0..=6 => between(rng, 1, 8),
                7..=8 => between(rng, RELEASE_TICKS - 3, RELEASE_TICKS + 3),
                9..=13 => between(rng, RELEASE_TICKS + 1, TICKS_PER_SECOND),
                _ => between(rng, TICKS_PER_SECOND, 30 * TICKS_PER_SECOND),
            }
        }
    }

    /// What one ADC channel reads for a while.
    #[derive(Clone, Copy, Debug)]
    enum Source {
        /// The same raw result every time.
        Steady(u16),
        /// That result, with one in four anything at all.
        Glitchy(u16),
        /// Anything at all, half of it more than the ADC can read.
        Garbage,
    }

    impl Source {
        fn read(self, rng: &mut Rng) -> u16 {
            match self {
                Self::Steady(raw) => raw,
                Self::Glitchy(raw) if rng.next_u16() % 4 != 0 => raw,
                Self::Glitchy(_) | Self::Garbage => match rng.next_u16() % 2 {
                    0 => rng.next_u16(),
                    _ => rng.next_u16() % (4 * 1023 + 1),
                },
            }
        }

        /// A cell that's good, middling, low, flat, or one the firmware
        /// can't read.
        fn cell(rng: &mut Rng) -> Self {
            match rng.next_u16() % 16 {
                0..=5 => Self::Steady(voltage_reading(between(rng, 340, 420))),
                6..=7 => Self::Steady(voltage_reading(between(rng, 300, 339))),
                8..=9 => Self::Steady(voltage_reading(between(rng, 291, 299))),
                10..=11 => Self::Steady(voltage_reading(between(rng, 150, 289))),
                12 => Self::Steady(match rng.next_u16() % 3 {
                    0 => 0,
                    1 => voltage_reading(60),
                    _ => 4 * 1023,
                }),
                13..=14 => Self::Glitchy(voltage_reading(between(rng, 300, 420))),
                _ => Self::Garbage,
            }
        }

        /// A head that's cool, hot, or one the sensor can't read.
        fn head(rng: &mut Rng) -> Self {
            let celsius = |degrees: u16| temperature_reading(thermal::celsius(degrees as u8));
            match rng.next_u16() % 8 {
                0..=3 => Self::Steady(celsius(between(rng, 0, 45))),
                4..=5 => Self::Steady(celsius(between(rng, 55, 95))),
                6 => Self::Glitchy(celsius(between(rng, 20, 95))),
                _ => Self::Garbage,
            }
        }
    }

    /// What `testing::EXAMPLES` says the clicks and holds that reach the UI have done
    /// so far, kept apart from `Ui` so the two can be compared.
    #[derive(Debug)]
    struct Model {
        mode: Mode,
        /// The level asked for while on.
        asked: u8,
        /// Whether the last 1H since the click on ramped up.
        up: bool,
        /// The tick the aux stops showing yellow or white.
        dark_at: u32,
        /// The tick lockout's red goes dark.
        red_until: u32,
        /// The last plausible reading with the light off was under 3.30 V,
        /// and none since was 3.40 V or more, so off's blue is dark.
        cell_low: bool,
        /// Ticks of rest until the next check of the cell, `0` meaning due.
        check_in: u16,
        /// The tick the last check of the cell started, `0` for none yet.
        checked_at: u32,
    }

    impl Model {
        /// One voltage reading taken with the light off, from the raw
        /// result the ADC gave.
        fn unloaded(&mut self, raw: u16) {
            let volts = battery::centivolts(raw);
            if !battery::PLAUSIBLE.contains(&volts) {
                return;
            }
            if volts < 330 {
                self.cell_low = true;
            } else if volts >= 340 {
                self.cell_low = false;
            }
        }
    }

    impl Model {
        fn handle(&mut self, event: Event) {
            match (self.mode, event) {
                (Mode::Off, Event::Click(1)) => {
                    self.mode = Mode::On;
                    self.asked = ON_LEVEL;
                    self.up = false;
                }
                (Mode::Off, Event::Click(3)) => self.mode = Mode::Lockout,
                (Mode::Lockout, Event::Click(3)) | (Mode::On, Event::Click(1)) => {
                    self.mode = Mode::Off;
                }
                (Mode::On, Event::Hold { clicks: 0, ticks }) => {
                    self.up ^= ticks == 0;
                    self.asked = match self.up {
                        true => (self.asked + 1).min(CEILING),
                        false => (self.asked - 1).max(ui::FLOOR),
                    };
                }
                _ => {}
            }
        }
    }

    /// How many ticks the noise spent in each state, so a change that stops
    /// it reaching one fails rather than quietly checking less.
    #[derive(Debug, Default)]
    struct Seen {
        lit: u32,
        at_the_top: u32,
        at_the_bottom: u32,
        held_down_by_the_cell: u32,
        held_down_by_heat: u32,
        locked: u32,
        resting: u32,
        flat: u32,
        faulted: u32,
        fading_out: u32,
        failed_at_power_on: u32,
        blue: u32,
        red: u32,
        yellow: u32,
        white: u32,
        results_lost: u32,
        turned_on: u32,
        went_flat: u32,
        charged: u32,
        went_blind: u32,
        clicked_off_mid_fade: u32,
        ignored_presses: u32,
        /// Ticks off's blue was dark for a low cell.
        blue_dark_for_the_cell: u32,
        darkened_by_a_press: u32,
        checks: u32,
        darkened_at_a_check: u32,
        relit_at_a_check: u32,
        garbage_at_a_check: u32,
        charged_at_a_check: u32,
    }

    /// One torch from power-on, for `ticks` ticks of random switching, cells,
    /// temperatures and ADC results, or until half a minute into a fault,
    /// checking every tick against `testing::EXAMPLES`. Returns how many ticks it ran.
    fn live(
        rng: &mut Rng,
        pace: Pace,
        faults: Faults,
        ticks: u32,
        seen: &mut Seen,
        life: u32,
    ) -> u32 {
        macro_rules! check {
            ($t:expr, $cond:expr, $($why:tt)+) => {
                assert!($cond, "life {life}, tick {}: {}", $t, format_args!($($why)+))
            };
        }
        let failed = faults.any();
        let mut rig = Rig::new(faults);
        let mut model = Model {
            mode: Mode::Off,
            asked: 0,
            up: false,
            dark_at: if failed { SHOW_TICKS.into() } else { 0 },
            red_until: 0,
            cell_low: false,
            check_in: 0,
            checked_at: 0,
        };
        // Slow torches start with the switch up half the time, so the first
        // tick's check gets a look in.
        let start_up = pace == Pace::Slow && rng.next_u16().is_multiple_of(2);
        let (mut pressed, mut switch_left) = (start_up, 0);
        let (mut cell, mut head) = (Source::Steady(3516), Source::Steady(2124));
        // One result in `lose` never arrives, and `0` is none of them.
        let (mut lose, mut world_left) = (0, 0);
        // Ticks the switch has been up, and ticks since the fault.
        let (mut up, mut faulted_for) = (0_u32, 0_u32);
        // The channels of the conversions started one and two ticks ago,
        // and whether the light was lit two ticks ago. Two voltage
        // conversions in a row are a turn, whose second is the reading.
        let (mut converted, mut converted_before) = (None, None);
        let mut lit_before_last = false;
        for t in 1..=ticks {
            if switch_left == 0 {
                pressed = !pressed;
                switch_left = span(rng, pressed, pace);
            }
            switch_left -= 1;
            if world_left == 0 {
                cell = Source::cell(rng);
                head = Source::head(rng);
                lose = match rng.next_u16() % 16 {
                    0..=11 => 0,
                    12..=14 => 4,
                    _ => 1,
                };
                world_left = match pace {
                    Pace::Slow if rng.next_u16().is_multiple_of(2) => {
                        between(rng, 60 * TICKS_PER_SECOND, 3 * CHECK_TICKS)
                    }
                    _ => between(rng, 1, 30 * TICKS_PER_SECOND),
                };
            }
            world_left -= 1;
            up = if pressed { 0 } else { up + 1 };

            let health_before = rig.torch.limits.health();
            let level_before = rig.level();
            let resting_before = rig.resting();
            let mut result = rig.adc.result();
            if lose != 0 && rng.next_u16().is_multiple_of(lose) {
                seen.results_lost += u32::from(result.take().is_some());
            }
            let event = rig.torch.tick(pressed, result);
            rig.adc
                .start_with(rig.torch.convert(), |channel| match channel {
                    Channel::Voltage => cell.read(rng),
                    Channel::Temperature => head.read(rng),
                });
            let health = rig.torch.limits.health();
            let level = rig.level();
            let convert = rig.convert();

            // The clicks and holds that should have reached the UI.
            let mut ignored = false;
            if let Some(event) = event {
                let press = matches!(event, Event::Click(_) | Event::Hold { ticks: 0, .. });
                if health_before == Health::Good || level_before > 0 {
                    model.handle(event);
                    if press && model.mode == Mode::Lockout {
                        model.red_until = t + u32::from(LOCKED_TICKS);
                    }
                } else if press {
                    model.dark_at = t + u32::from(SHOW_TICKS);
                    ignored = true;
                }
            }
            // The reading that came in this tick, if the last two ticks
            // started a voltage turn's two conversions. It's the cell with
            // nothing on it if the UI's off now and the light was out for
            // both conversions' ticks, and then it's what decides off's blue.
            let at_a_check = model.checked_at != 0 && t == model.checked_at + 2 && up >= 3;
            let voltage = Some(Channel::Voltage);
            if converted == voltage && converted_before == voltage {
                let unloaded = model.mode != Mode::On && level_before == 0 && !lit_before_last;
                if unloaded {
                    let low = model.cell_low;
                    model.unloaded(result.unwrap_or(0));
                    let plausible = battery::plausible(battery::centivolts(result.unwrap_or(0)));
                    if at_a_check {
                        seen.darkened_at_a_check += u32::from(!low && model.cell_low);
                        seen.relit_at_a_check += u32::from(low && !model.cell_low);
                        seen.garbage_at_a_check += u32::from(!plausible);
                    } else {
                        seen.darkened_by_a_press += u32::from(!low && model.cell_low);
                    }
                }
            } else {
                check!(t, !at_a_check, "a check with no reading");
            }
            converted_before = converted;
            converted = convert.map(|c| c.channel);
            lit_before_last = level_before > 0;
            if model.mode == Mode::On && level == 0 {
                check!(t, health != Health::Good, "off with nothing wrong");
                model.mode = Mode::Off;
            }
            if health != Health::Good && (health_before == Health::Good || level_before > 0) {
                model.dark_at = t + u32::from(SHOW_TICKS);
            }
            check!(t, rig.torch.ui.mode() == model.mode, "{model:?}");
            let asked = match model.mode {
                Mode::On => model.asked,
                Mode::Off | Mode::Lockout => 0,
            };
            check!(t, rig.torch.ui.output_level() == asked, "{model:?}");

            // The level.
            check!(t, level <= CEILING, "level {level}");
            check!(t, rig.output() == ramp::output(level), "level {level}");
            check!(t, level <= asked, "level {level} of {asked}");
            check!(t, (level > 0) == (model.mode == Mode::On), "level {level}");
            check!(t, !failed || level == 0, "lit after a failed check");
            let moved = level.abs_diff(level_before);
            let click = event == Some(Event::Click(1));
            if level_before == 0 && level > 0 {
                check!(t, click, "on without a click: {event:?}");
                check!(t, health_before == Health::Good, "{health_before:?}");
                check!(t, health == Health::Good, "{health:?}");
                check!(t, level <= ON_LEVEL, "on at {level}");
                seen.turned_on += 1;
            } else if level_before > 0 && level == 0 {
                check!(
                    t,
                    click || health != Health::Good,
                    "went out with nothing wrong and no click"
                );
            } else {
                // A level a tick at most, even when the ramp and a ceiling
                // are both on their way down.
                check!(t, moved <= 1, "{level_before} to {level}");
            }

            // What's wrong.
            match (health_before, health) {
                (Health::Good, Health::Good)
                | (Health::Flat, Health::Flat)
                | (Health::Fault, Health::Fault) => {}
                (Health::Good, Health::Flat) => {
                    check!(t, level_before == 1, "flat from level {level_before}");
                    seen.went_flat += 1;
                }
                (Health::Flat, Health::Good) => {
                    let volts = result.map(battery::centivolts);
                    check!(
                        t,
                        volts.is_some_and(|v| v >= battery::CHARGED_AT && battery::plausible(v)),
                        "charged by {volts:?}"
                    );
                    seen.charged += 1;
                }
                (Health::Good | Health::Flat, Health::Fault) => seen.went_blind += 1,
                (Health::Fault, _) => check!(t, false, "a fault cleared to {health:?}"),
            }
            check!(t, !failed || health == Health::Fault, "{health:?}");
            if health == Health::Flat {
                check!(t, level == 0, "flat at level {level}");
            }
            if health_before == Health::Fault {
                // Down a level a tick, then off, or off at a click.
                check!(t, level <= level_before, "{level_before} to {level}");
                if level > 0 {
                    check!(t, moved == 1, "{level_before} to {level}");
                } else if level_before > 0 {
                    check!(t, click || level_before == 1, "out from {level_before}");
                    seen.clicked_off_mid_fade += u32::from(click);
                }
            }

            // The aux.
            let aux = rig.aux();
            check!(t, level == 0 || aux == Aux::Off, "{aux:?} at level {level}");
            let showing = t < model.dark_at;
            let expected = match (health, model.mode) {
                _ if level > 0 => Aux::Off,
                (Health::Good, Mode::Off) if !model.cell_low => Aux::Blue,
                (Health::Good, Mode::Lockout) if t < model.red_until => Aux::Red,
                (Health::Flat, _) if showing => Aux::Yellow,
                (Health::Fault, _) if showing => Aux::White,
                _ => Aux::Off,
            };
            check!(t, aux == expected, "{aux:?}, {health:?}, {model:?}");

            // Resting.
            let resting = rig.resting();
            if resting {
                check!(t, level == 0, "resting at level {level}");
                check!(t, rig.convert().is_none(), "resting with the ADC busy");
            }
            // A check of the cell is due after enough rest, never with a
            // fault, and takes two ticks awake before it rests again.
            if resting_before && !pressed {
                check!(t, event.is_none(), "an event with the switch up");
                if model.check_in == 0 && health_before != Health::Fault {
                    let started = Some(Convert {
                        channel: Channel::Voltage,
                        setup: true,
                    });
                    check!(t, !resting && convert == started, "no check, {convert:?}");
                    model.checked_at = t;
                    seen.checks += 1;
                } else {
                    check!(t, resting, "woke with the switch up");
                    check!(t, health == health_before, "{health:?} while resting");
                }
            }
            let checking =
                model.checked_at != 0 && t - model.checked_at <= 2 && up > t - model.checked_at;
            if checking {
                check!(t, level == 0, "lit by a check");
                check!(
                    t,
                    health == health_before
                        || (health_before == Health::Flat && health == Health::Good),
                    "a check took it from {health_before:?} to {health:?}"
                );
                check!(t, resting == at_a_check, "two ticks awake, then rest");
                if at_a_check && (health_before, health) == (Health::Flat, Health::Good) {
                    seen.charged_at_a_check += 1;
                }
            }
            model.check_in = match resting {
                true => model.check_in.saturating_sub(1),
                false => CHECK_TICKS,
            };
            if resting_before && pressed {
                check!(t, rig.convert().is_some(), "a press that reads nothing");
            }
            if up > u32::from(RELEASE_TICKS) + 2 && level == 0 && !checking {
                check!(t, resting, "nothing to do and not resting");
            }
            seen.blue_dark_for_the_cell +=
                u32::from(health == Health::Good && model.mode == Mode::Off && model.cell_low);

            seen.lit += u32::from(level > 0);
            seen.at_the_top += u32::from(level == CEILING);
            seen.at_the_bottom += u32::from(level == ui::FLOOR && asked == ui::FLOOR);
            if health == Health::Good && level > 0 && level < asked {
                let (cell, heat) = rig.torch.limits.ceilings();
                seen.held_down_by_the_cell += u32::from(level == cell);
                seen.held_down_by_heat += u32::from(level == heat);
            }
            seen.locked += u32::from(model.mode == Mode::Lockout);
            seen.resting += u32::from(resting);
            seen.flat += u32::from(health == Health::Flat);
            seen.faulted += u32::from(health == Health::Fault && !failed);
            seen.fading_out += u32::from(health == Health::Fault && level > 0);
            seen.failed_at_power_on += u32::from(failed);
            seen.blue += u32::from(aux == Aux::Blue);
            seen.red += u32::from(aux == Aux::Red);
            seen.yellow += u32::from(aux == Aux::Yellow);
            seen.white += u32::from(aux == Aux::White);
            seen.ignored_presses += u32::from(ignored);

            // Nothing changes a fault, so it gets long enough for the aux to
            // go dark and be woken a few times, and no longer.
            faulted_for = if health == Health::Fault {
                faulted_for + 1
            } else {
                0
            };
            if faulted_for > 30 * u32::from(TICKS_PER_SECOND) {
                return t;
            }
        }
        ticks
    }

    /// Torches from power-on, one after another, until they've run for
    /// `ticks` ticks between them. One in eight failed a power-on check. A
    /// fault lasts until the cell comes out, so each gets a few minutes at
    /// most. At [`Pace::Slow`] each lives ten times as long.
    fn throw_noise_at_torches(seed: u32, pace: Pace, ticks: u32, seen: &mut Seen) {
        let mut rng = Rng::new(seed);
        let (mut life, mut run) = (0, 0);
        while run < ticks {
            let faults = match rng.next_u16() % 8 {
                0 => Faults {
                    hang: true,
                    ..Faults::NONE
                },
                _ => Faults::NONE,
            };
            let mut length = u32::from(between(&mut rng, 1000, 20_000));
            if pace == Pace::Slow {
                length *= 10;
            }
            run += live(&mut rng, pace, faults, length, seen, life);
            life += 1;
        }
    }

    /// Eight million ticks, about a day and a half of torch, with rests of
    /// up to a quarter of an hour, so the cell's checked at rest hundreds of
    /// times, through every kind of reading.
    #[test]
    fn random_switching_with_long_rests_never_breaks_a_promise() {
        let mut seen = Seen::default();
        for seed in [0x5EED_0005, 0x5EED_0006] {
            throw_noise_at_torches(seed, Pace::Slow, 4_000_000, &mut seen);
        }
        for (what, times) in [
            ("resting", seen.resting),
            ("blue", seen.blue),
            ("blue dark for the cell", seen.blue_dark_for_the_cell),
            ("checking the cell", seen.checks),
            ("darkened at a check", seen.darkened_at_a_check),
            ("relit at a check", seen.relit_at_a_check),
            ("reading garbage at a check", seen.garbage_at_a_check),
            ("charged at a check", seen.charged_at_a_check),
            ("darkened by a press", seen.darkened_by_a_press),
            ("locked out", seen.locked),
            ("flat", seen.flat),
            ("failed at power-on", seen.failed_at_power_on),
        ] {
            assert!(times > 0, "never {what}: {seen:?}");
        }
    }

    /// Two million ticks, about nine hours of torch, and every state in
    /// `testing::EXAMPLES` reached along the way.
    #[test]
    fn random_switching_and_readings_never_break_a_promise() {
        let mut seen = Seen::default();
        for seed in [0x5EED_0003, 0x5EED_0004] {
            throw_noise_at_torches(seed, Pace::Quick, 1_000_000, &mut seen);
        }
        for (what, times) in [
            ("blue dark for the cell", seen.blue_dark_for_the_cell),
            ("darkened by a press", seen.darkened_by_a_press),
            ("lit", seen.lit),
            ("at the top of the ramp", seen.at_the_top),
            ("at the bottom of the ramp", seen.at_the_bottom),
            ("held down by the cell", seen.held_down_by_the_cell),
            ("held down by heat", seen.held_down_by_heat),
            ("locked out", seen.locked),
            ("resting", seen.resting),
            ("flat", seen.flat),
            ("faulted", seen.faulted),
            ("fading out for a fault", seen.fading_out),
            ("failed at power-on", seen.failed_at_power_on),
            ("blue", seen.blue),
            ("red", seen.red),
            ("yellow", seen.yellow),
            ("white", seen.white),
            ("missing a result", seen.results_lost),
            ("turned on", seen.turned_on),
            ("gone flat", seen.went_flat),
            ("charged", seen.charged),
            ("unable to read the cell", seen.went_blind),
            ("clicked off mid-fade", seen.clicked_off_mid_fade),
            ("pressed with the light kept out", seen.ignored_presses),
        ] {
            assert!(times > 0, "never {what}: {seen:?}");
        }
    }

    #[test]
    fn the_first_output_always_counts_as_a_change() {
        assert_eq!(Changes::new().update(Output::OFF), Some(Output::OFF));
        assert_eq!(Changes::new().update(dim()), Some(dim()));
    }

    #[test]
    fn turning_on_and_off_are_changes_and_staying_put_is_not() {
        let mut changes = Changes::new();
        assert_eq!(changes.update(Output::OFF), Some(Output::OFF));
        assert_eq!(changes.update(Output::OFF), None);
        assert_eq!(changes.update(dim()), Some(dim()));
        assert_eq!(changes.update(dim()), None);
        assert_eq!(changes.update(Output::OFF), Some(Output::OFF));
    }
}
