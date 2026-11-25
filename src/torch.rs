use crate::TICKS_PER_SECOND;
use crate::button::{Clicks, Debounce, Event};
use crate::limits::{Health, Limits};
use crate::ramp::{self, Output};
use crate::selftest::Faults;
use crate::sensors::Convert;
use crate::temp::Calibration;
use crate::ui::{Aux, Mode, Ui};

// All five are unmeasured on this torch.
pub const SHOW_TICKS: u16 = 3 * TICKS_PER_SECOND;
pub const LOCKED_TICKS: u16 = 5 * TICKS_PER_SECOND;
pub const CHECK_TICKS: u16 = 5 * 60 * TICKS_PER_SECOND;
pub const AUX_DARK_BELOW: u16 = 330;
pub const AUX_LIT_FROM: u16 = 340;

const _: () = assert!(AUX_DARK_BELOW < AUX_LIT_FROM);
const _: () = assert!(
    *crate::battery::PLAUSIBLE.start() < AUX_DARK_BELOW
        && AUX_LIT_FROM < *crate::battery::PLAUSIBLE.end(),
    "both are centivolts, as the cell's readings are"
);
const _: () = assert!(crate::battery::FORGET_TICKS <= CHECK_TICKS);

/// `repr(C)` holds `limits` last, since the firmware's loop keeps a `Torch` on
/// its stack and `ldd` and `std` only reach 63 bytes up it.
#[derive(Clone, Debug)]
#[repr(C)]
pub struct Torch {
    switch: Debounce,
    clicks: Clicks,
    ui: Ui,
    showing: u16,
    check_in: u16,
    checking: bool,
    cell_low: bool,
    limits: Limits,
}

impl Torch {
    #[must_use]
    pub const fn new(calibration: Calibration, faults: Faults) -> Self {
        Self {
            switch: Debounce::new(),
            clicks: Clicks::new(),
            ui: Ui::new(),
            limits: Limits::new(calibration, faults),
            showing: if faults.any() { SHOW_TICKS } else { 0 },
            check_in: 0,
            checking: false,
            cell_low: false,
        }
    }

    pub fn tick(&mut self, pressed: bool, adc: Option<u16>, ticks: u16) -> Option<Event> {
        self.showing = self.showing.saturating_sub(1);
        let down = self.switch.sample(pressed);
        let event = self.clicks.tick_counting(down, self.ui.count());
        if let Some(event) = event {
            let press = matches!(event, Event::Click(_) | Event::Hold { ticks: 0, .. });
            if self.limits.health() == Health::Good || self.level() > 0 {
                self.ui.handle(event);
                if press && self.ui.mode() == Mode::Lockout {
                    self.showing = LOCKED_TICKS;
                }
            } else {
                self.ui.handle_unlit(event);
                if press {
                    self.showing = SHOW_TICKS;
                }
            }
        }
        let before = self.limits.health();
        let lit = self.level() > 0;
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
                Some(volts) if volts < AUX_DARK_BELOW => self.cell_low = true,
                Some(volts) if volts >= AUX_LIT_FROM => self.cell_low = false,
                _ => {}
            }
            if self.limits.convert().is_none() {
                self.checking = false;
            }
        }
        let health = self.limits.health();
        if health != Health::Good && (before == Health::Good || lit) {
            self.showing = SHOW_TICKS;
        }
        if self.resting() {
            self.limits.rest(ticks);
            self.check_in = self.check_in.saturating_sub(ticks);
        } else {
            self.check_in = CHECK_TICKS;
        }
        event
    }

    /// Inlined, each caller in the firmware's loop gets a copy, about 230 bytes of flash.
    #[inline(never)]
    const fn quiet(&self) -> bool {
        self.switch.settled()
            && !self.clicks.counting()
            && self.ui.output_level() == 0
            && self.limits.level() == 0
    }

    #[must_use]
    pub const fn resting(&self) -> bool {
        self.quiet() && !self.checking
    }

    #[must_use]
    pub const fn slow(&self) -> bool {
        self.resting() && self.clicks.idle() && self.showing == 0
    }

    #[must_use]
    pub const fn level(&self) -> u8 {
        self.limits.level()
    }

    #[must_use]
    pub const fn convert(&self) -> Option<Convert> {
        self.limits.convert()
    }

    #[must_use]
    pub fn output(&self) -> Output {
        let charge = match self.clicks.pending(self.ui.count()) {
            Some(event) if self.limits.health() == Health::Good && !self.limits.weak() => {
                self.ui.lights(event)
            }
            _ => 0,
        };
        match self.level() {
            0 => Output {
                enable: false,
                dsm: ramp::output(charge).dsm,
            },
            _ => ramp::faded(self.limits.position()),
        }
    }

    #[must_use]
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

#[derive(Clone, Copy, Debug, Default)]
pub struct Changes<T> {
    last: Option<T>,
}

impl<T: Copy + PartialEq> Changes<T> {
    #[must_use]
    pub const fn new() -> Self {
        Self { last: None }
    }

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
    use crate::button::{CLICK_TICKS, GUARD_TICKS, HOLD_TICKS, RELEASE_TICKS};
    use crate::ramp::CEILING;
    use crate::sensors::{Channel, FULL_SCALE};
    use crate::testing::{
        EXAMPLES, Rig, Rng, Scene, example, temperature_reading, voltage_reading,
    };
    use crate::thermal;
    use crate::ui::{self, Mode, ON_LEVEL, TWO_H};

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

    fn at_the_top(torch: &mut Rig) {
        click(torch);
        run(torch, true, 3 * TICKS_PER_SECOND);
        run(torch, false, RELEASE_TICKS + 1);
        assert_eq!(torch.level(), CEILING);
    }

    fn shows_for(torch: &mut Rig, aux: Aux, ticks: u16) {
        for tick in 0..ticks {
            assert_eq!(torch.aux(), aux, "tick {tick}");
            torch.tick(false);
        }
        assert_eq!(torch.aux(), Aux::Off);
    }

    fn until_fault(torch: &mut Rig) {
        while torch.limits.health() == Health::Good {
            torch.tick(false);
        }
    }

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
    fn a_click_turns_it_on_as_it_is_let_go() {
        let mut torch = Rig::default();
        run(&mut torch, true, 4);
        torch.tick(false);
        assert_eq!(torch.level(), 0);
        assert_eq!(torch.tick(false), Some(Event::Click(1)));
        assert_eq!(torch.output(), dim());
    }

    fn charging(level: u8) -> Output {
        Output {
            enable: false,
            dsm: ramp::output(level).dsm,
        }
    }

    #[test]
    fn a_press_from_off_runs_the_pwm_with_pb2_low_at_what_it_would_light() {
        let mut torch = Rig::default();
        run(&mut torch, false, TICKS_PER_SECOND);
        torch.tick(true);
        assert_eq!(torch.output(), Output::OFF, "one sample");
        for tick in 0..CLICK_TICKS {
            torch.tick(true);
            assert_eq!(torch.output(), charging(ON_LEVEL), "a click, tick {tick}");
        }
        for tick in CLICK_TICKS..HOLD_TICKS {
            torch.tick(true);
            assert_eq!(torch.output(), charging(ui::FLOOR), "a hold, tick {tick}");
        }
        torch.tick(true);
        assert_eq!(torch.level(), ui::FLOOR);
    }

    #[test]
    fn after_a_click_off_a_press_charges_nothing_inside_the_guard_and_the_click_on_after() {
        let mut torch = Rig::default();
        click(&mut torch);
        run(&mut torch, true, 4);
        run(&mut torch, false, 2);
        assert_eq!(torch.level(), 0);
        for pressed in [true, true, true, false, false, true, true, true] {
            torch.tick(pressed);
            assert_eq!(torch.output(), Output::OFF);
        }
        run(&mut torch, false, GUARD_TICKS + 1);
        run(&mut torch, true, 2);
        assert_eq!(torch.output(), charging(ON_LEVEL));
    }

    #[test]
    fn the_pwm_runs_straight_on_into_the_light() {
        for (press, level) in [(4, ON_LEVEL), (HOLD_TICKS + 2, ui::FLOOR)] {
            let mut torch = Rig::default();
            run(&mut torch, false, TICKS_PER_SECOND);
            run(&mut torch, true, press);
            run(&mut torch, false, RELEASE_TICKS + 2);
            assert_eq!(torch.output(), ramp::output(level), "{press}");
        }
    }

    #[test]
    fn nothing_charges_in_lockout_with_a_fault_or_on_a_weak_cell() {
        let mut torch = Rig::default();
        lock(&mut torch);
        run(&mut torch, false, RELEASE_TICKS + 1);
        run(&mut torch, true, 4);
        assert_eq!(torch.output(), Output::OFF, "locked");

        let mut torch = Rig::new(Faults {
            fuses: true,
            ..Faults::NONE
        });
        run(&mut torch, true, 4);
        assert_eq!(torch.output(), Output::OFF, "a fault");

        let mut torch = Rig::default();
        torch.adc.voltage = voltage_reading(295);
        run(&mut torch, false, TICKS_PER_SECOND);
        run(&mut torch, true, 4);
        assert_eq!(torch.output(), Output::OFF, "a weak cell");
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
        lock(&mut torch);
        run(&mut torch, false, RELEASE_TICKS + 1);
        run(&mut torch, true, 2 * TICKS_PER_SECOND);
        assert_eq!(torch.level(), 0);
        run(&mut torch, false, RELEASE_TICKS + 1);
        click(&mut torch);
        assert_eq!(torch.level(), 0);
        assert_eq!(torch.aux(), Aux::Red);
    }

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

    fn two_h(torch: &mut Rig) {
        clicks(torch, 1);
        let held = (0..2 * HOLD_TICKS).any(|_| torch.tick(true) == Some(TWO_H));
        assert!(held, "no 2H");
    }

    fn lock(torch: &mut Rig) {
        two_h(torch);
        assert_eq!(torch.torch.ui.mode(), Mode::Lockout);
        run(torch, false, RELEASE_TICKS + 1);
    }

    #[test]
    fn a_2h_with_the_light_on_puts_it_out_at_the_click_and_locks_it_at_the_hold() {
        let mut torch = Rig::default();
        click(&mut torch);
        run(&mut torch, true, 4);
        torch.tick(false);
        assert_eq!(torch.tick(false), Some(Event::Click(1)), "off, from on");
        assert_eq!(torch.level(), 0);
        run(&mut torch, false, GUARD_TICKS);
        let event = (0..2 * HOLD_TICKS).find_map(|_| torch.tick(true));
        assert_eq!(event, Some(TWO_H));
        assert_eq!(torch.torch.ui.mode(), Mode::Lockout);
        assert_eq!(torch.level(), 0);
    }

    #[test]
    fn the_light_stays_out_all_the_way_through_locking_it() {
        let mut torch = Rig::default();
        click(&mut torch);
        run(&mut torch, true, 4);
        run(&mut torch, false, 2);
        for (pressed, ticks) in [
            (false, 2),
            (true, 2 * HOLD_TICKS),
            (false, TICKS_PER_SECOND),
        ] {
            for _ in 0..ticks {
                torch.tick(pressed);
                assert_eq!(torch.level(), 0);
                assert!(!torch.output().enable);
            }
        }
        assert_eq!(torch.torch.ui.mode(), Mode::Lockout);
    }

    #[test]
    fn a_2h_from_off_lights_it_until_the_hold_and_then_locks_it() {
        let mut torch = Rig::default();
        run(&mut torch, true, 4);
        run(&mut torch, false, 4);
        assert_eq!(torch.level(), ON_LEVEL);
        let event = (0..2 * HOLD_TICKS).find_map(|_| torch.tick(true));
        assert_eq!(event, Some(TWO_H));
        assert_eq!(torch.level(), 0);
        assert_eq!(torch.torch.ui.mode(), Mode::Lockout);
    }

    #[test]
    fn each_click_turns_it_on_or_off_however_quick() {
        for n in 1..=6u8 {
            let mut torch = Rig::default();
            clicks(&mut torch, n);
            let on = !n.is_multiple_of(2);
            assert_eq!(torch.level(), if on { ON_LEVEL } else { 0 }, "{n}C");
            run(&mut torch, false, TICKS_PER_SECOND);
            let mode = if on { Mode::On } else { Mode::Off };
            assert_eq!(torch.torch.ui.mode(), mode, "{n}C");
        }
    }

    #[test]
    fn a_click_read_twice_inside_the_guard_turns_it_off_and_leaves_it_off() {
        for again in 1..=3 * HOLD_TICKS {
            let mut torch = Rig::default();
            click(&mut torch);
            run(&mut torch, true, 4);
            run(&mut torch, false, GUARD_TICKS - 1);
            run(&mut torch, true, again);
            run(&mut torch, false, TICKS_PER_SECOND);
            assert_eq!(torch.level(), 0, "{again} down");
            assert_eq!(torch.torch.ui.mode(), Mode::Off, "{again} down");
        }
    }

    fn until_resting(torch: &mut Rig) {
        for _ in 0..TICKS_PER_SECOND {
            if torch.resting() {
                return;
            }
            torch.tick(false);
        }
        panic!("never rested");
    }

    #[test]
    fn a_press_from_rest_counts_once_a_second_tick_sees_it() {
        for (samples, lights) in [(1, false), (2, true)] {
            let mut torch = Rig::default();
            until_resting(&mut torch);
            run(&mut torch, false, 5);
            assert!(torch.resting());
            run(&mut torch, true, samples);
            run(&mut torch, false, TICKS_PER_SECOND);
            assert_eq!(torch.level() > 0, lights, "down for {samples} ticks");
        }
    }

    #[test]
    fn one_sample_of_the_switch_changes_nothing() {
        let mut torch = Rig::default();
        until_resting(&mut torch);
        assert_eq!(torch.tick(true), None);
        assert!(!torch.resting(), "awake to look again");
        run(&mut torch, false, TICKS_PER_SECOND);
        assert!(torch.resting());
        assert_eq!(torch.level(), 0);

        click(&mut torch);
        torch.tick(true);
        run(&mut torch, false, TICKS_PER_SECOND);
        assert_eq!(torch.level(), ON_LEVEL, "one sample down while lit");

        run(&mut torch, true, 4);
        torch.tick(false);
        run(&mut torch, true, 4);
        run(&mut torch, false, TICKS_PER_SECOND);
        assert_eq!(torch.level(), 0, "one sample up in a click is one click");

        click(&mut torch);
        run(&mut torch, true, 40);
        torch.tick(false);
        run(&mut torch, true, 40);
        assert_eq!(
            u16::from(torch.level()),
            u16::from(ON_LEVEL) + 80 - HOLD_TICKS,
            "one sample up in a hold is one hold"
        );
    }

    #[test]
    fn four_clicks_in_lockout_neither_unlock_it_nor_light_it() {
        let mut torch = Rig::default();
        lock(&mut torch);
        run(&mut torch, false, RELEASE_TICKS + 1);
        tap(&mut torch, 4, Event::Click(4));
        assert_eq!(torch.torch.ui.mode(), Mode::Lockout);
        run(&mut torch, false, TICKS_PER_SECOND);
        assert_eq!(torch.level(), 0);
    }

    #[test]
    fn lockout_shows_bright_red_for_five_seconds_after_each_press() {
        let mut torch = Rig::default();
        two_h(&mut torch);
        assert_eq!(torch.aux(), Aux::Red, "as 2H locks it");
        shows_for(&mut torch, Aux::Red, LOCKED_TICKS);
        tap(&mut torch, 1, Event::Click(1));
        assert_eq!(torch.aux(), Aux::Red, "a press that does nothing");
        run(&mut torch, false, LOCKED_TICKS);
        assert_eq!(torch.aux(), Aux::Off);
        two_h(&mut torch);
        assert_eq!(torch.aux(), Aux::Blue, "2H unlocks it");
    }

    #[test]
    fn a_hold_in_lockout_cannot_keep_the_red_lit() {
        let mut torch = Rig::default();
        lock(&mut torch);
        run(&mut torch, false, LOCKED_TICKS);
        assert_eq!(torch.aux(), Aux::Off);
        run(&mut torch, true, HOLD_TICKS + 2);
        assert_eq!(torch.aux(), Aux::Red, "from the start of the hold");
        run(&mut torch, true, LOCKED_TICKS);
        assert_eq!(torch.aux(), Aux::Off, "still held");
    }

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
            rest(&mut torch, CHECK_TICKS - 1, why);
            assert_a_check(&mut torch, why);
        }
    }

    #[test]
    fn a_press_or_the_light_starts_the_five_minutes_again() {
        let mut torch = Rig::default();
        run(&mut torch, false, 3);
        rest(&mut torch, CHECK_TICKS / 2, "half way");
        click(&mut torch);
        run(&mut torch, false, 10 * TICKS_PER_SECOND);
        tap(&mut torch, 1, Event::Click(1));
        assert_eq!(torch.level(), 0);
        assert!(torch.resting(), "out as 1C's let go");
        rest(&mut torch, CHECK_TICKS - 1, "after the light");
        assert_a_check(&mut torch, "five minutes after the light");
        lock(&mut torch);
        rest(&mut torch, CHECK_TICKS / 2, "half way");
        tap(&mut torch, 2, Event::Click(2));
        while !torch.resting() {
            torch.tick(false);
        }
        rest(&mut torch, CHECK_TICKS - 1, "after the press");
        assert_a_check(&mut torch, "five minutes after the press");
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
        run(&mut torch, true, 4);
        assert_eq!(torch.aux(), Aux::Off, "pressed");
        run(&mut torch, false, RELEASE_TICKS + 1);
        assert_eq!(torch.level(), ON_LEVEL);
        click(&mut torch);
        assert_eq!(torch.aux(), Aux::Off, "read low by the press that lit it");
        torch.adc.voltage = voltage_reading(380);
        click(&mut torch);
        click(&mut torch);
        assert_eq!(torch.aux(), Aux::Blue, "and read again by the next");
    }

    #[test]
    fn readings_with_the_light_on_never_move_the_aux() {
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
        lock(&mut torch);
        run(&mut torch, false, LOCKED_TICKS);
        assert_eq!(torch.aux(), Aux::Off, "red for five seconds only");
        torch.adc.voltage = voltage_reading(320);
        for _ in 0..3 {
            next_check(&mut torch);
            assert_eq!(torch.torch.ui.mode(), Mode::Lockout);
            assert_eq!(torch.level(), 0);
            assert_eq!(torch.aux(), Aux::Off);
        }
        tap(&mut torch, 1, Event::Click(1));
        shows_for(&mut torch, Aux::Red, LOCKED_TICKS);
        two_h(&mut torch);
        run(&mut torch, false, RELEASE_TICKS + 1);
        assert_eq!(torch.torch.ui.mode(), Mode::Off);
        assert_eq!(torch.aux(), Aux::Off);
    }

    #[test]
    fn garbage_at_rest_leaves_the_aux_alone_and_is_no_fault() {
        for garbage in [0, voltage_reading(60), FULL_SCALE] {
            let mut torch = Rig::default();
            torch.adc.voltage = voltage_reading(320);
            next_check(&mut torch);
            assert_eq!(torch.aux(), Aux::Off, "{garbage}");
            torch.adc.voltage = garbage;
            for _ in 0..12 {
                next_check(&mut torch);
                assert_eq!(torch.aux(), Aux::Off, "{garbage}");
                assert_eq!(torch.limits.health(), Health::Good, "{garbage}");
            }
            torch.adc.voltage = voltage_reading(380);
            next_check(&mut torch);
            assert_eq!(torch.aux(), Aux::Blue, "{garbage}");
            torch.adc.voltage = garbage;
            for _ in 0..12 {
                next_check(&mut torch);
                assert_eq!(torch.aux(), Aux::Blue, "{garbage}");
                assert_eq!(torch.limits.health(), Health::Good, "{garbage}");
            }
            torch.adc.voltage = voltage_reading(380);
            click(&mut torch);
            assert_eq!(torch.level(), ON_LEVEL, "{garbage}");
            click(&mut torch);
            torch.adc.voltage = garbage;
            run(&mut torch, true, 2 * TICKS_PER_SECOND);
            assert_eq!(torch.limits.health(), Health::Fault, "{garbage}");
        }
    }

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
            assert_eq!(rig.limits.health(), Health::Good, "{row}");
        }
        let mut low = Rig::play(example("Low cell, at rest").actions().take(2));
        assert_eq!(low.aux(), Aux::Off, "dark before the 1C");
        low.adc.voltage = voltage_reading(320);
        click(&mut low);
        assert_eq!(low.level(), ON_LEVEL);
        click(&mut low);
        assert_eq!(low.aux(), Aux::Off, "and once it's off again");
        let garbage = Rig::play(example("A cell it can't read, at rest").actions().take(2));
        assert_eq!(garbage.aux(), Aux::Blue, "blue through the garbage");
        assert_eq!(garbage.limits.health(), Health::Good, "no fault yet");
    }

    #[test]
    fn a_check_whose_result_never_comes_still_ends() {
        let mut torch = Rig::default();
        run(&mut torch, false, 3);
        torch.adc.voltage = voltage_reading(320);
        next_check(&mut torch);
        assert_eq!(torch.aux(), Aux::Off);
        rest(&mut torch, CHECK_TICKS - 1, "to the next check");
        for _ in 0..2 {
            torch.torch.tick(false, None, 1);
            assert!(!torch.resting());
        }
        torch.torch.tick(false, None, 1);
        assert!(torch.resting());
        assert_eq!(torch.convert(), None);
        assert_eq!(torch.aux(), Aux::Off);
        assert_eq!(torch.limits.health(), Health::Good);
    }

    #[test]
    fn a_flat_cell_charged_at_rest_is_good_again_at_the_next_check() {
        let mut torch = Rig::default();
        click(&mut torch);
        torch.adc.voltage = voltage_reading(280);
        run(&mut torch, false, 10 * TICKS_PER_SECOND);
        assert_eq!(torch.limits.health(), Health::Flat);
        torch.adc.voltage = voltage_reading(350);
        for _ in 0..3 {
            next_check(&mut torch);
            assert_eq!(torch.limits.health(), Health::Flat);
            assert_eq!(torch.aux(), Aux::Off, "yellow only after a press");
        }
        torch.adc.voltage = voltage_reading(380);
        let ticks = next_check(&mut torch);
        assert!(ticks <= u32::from(CHECK_TICKS) + 3, "{ticks}");
        assert_eq!(torch.limits.health(), Health::Good, "with no press");
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
        let mut torch = Rig::default();
        click(&mut torch);
        torch.adc.voltage = 0;
        run(&mut torch, false, 5 * TICKS_PER_SECOND);
        assert_eq!(torch.limits.health(), Health::Fault);
        assert!(torch.resting());
        for tick in 0..2 * u32::from(CHECK_TICKS) {
            torch.tick(false);
            assert!(torch.resting(), "tick {tick}");
        }
    }

    #[test]
    fn a_conversion_running_as_a_hold_ends_is_read_before_it_rests() {
        let (mut read, mut not) = (0, 0);
        for down in HOLD_TICKS + 1..HOLD_TICKS + 17 {
            let mut torch = Rig::default();
            lock(&mut torch);
            run(&mut torch, false, LOCKED_TICKS);
            torch.adc.voltage = voltage_reading(320);
            run(&mut torch, true, down);
            assert!(torch.torch.cell_low, "{down}");
            torch.adc.voltage = voltage_reading(380);
            torch.tick(false);
            let running = torch
                .convert()
                .is_some_and(|c| c.channel == Channel::Voltage);
            assert_eq!(torch.tick(false), Some(Event::Release { clicks: 0 }));
            run(&mut torch, false, 2);
            assert!(torch.resting(), "{down}");
            assert_eq!(torch.torch.cell_low, !running, "{down}");
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
    fn ticks_once_a_second_only_with_nothing_counting_ticks() {
        let mut torch = Rig::default();
        run(&mut torch, false, TICKS_PER_SECOND);
        assert!(torch.slow());
        click(&mut torch);
        run(&mut torch, true, 4);
        run(&mut torch, false, 2);
        assert_eq!(torch.level(), 0);
        assert!(
            torch.resting() && !torch.slow(),
            "the release after a click that acted"
        );
        run(&mut torch, false, RELEASE_TICKS);
        assert!(torch.slow());
        lock(&mut torch);
        run(&mut torch, false, RELEASE_TICKS + 1);
        assert!(!torch.slow(), "lockout's red");
        run(&mut torch, false, LOCKED_TICKS);
        assert!(torch.slow());
    }

    #[test]
    fn a_rest_tick_counts_a_second_towards_the_next_check() {
        let mut torch = Rig::default();
        run(&mut torch, false, TICKS_PER_SECOND);
        let mut seconds = 1;
        while torch.resting() {
            torch.torch.tick(false, None, TICKS_PER_SECOND);
            seconds += 1;
            assert!(seconds <= 302, "no check");
        }
        assert!(torch.convert().is_some());
        assert_eq!(
            seconds,
            CHECK_TICKS / TICKS_PER_SECOND + 2,
            "up to a rest tick late, then the rest tick that starts it"
        );
    }

    fn paced(torch: &mut Rig, pressed: bool, ticks: u16) {
        for _ in 0..ticks {
            torch.tick_in(pressed, &Scene::NORMAL);
        }
    }

    fn at_rest() -> Rig {
        let mut torch = Rig::default();
        paced(&mut torch, false, TICKS_PER_SECOND);
        assert!(torch.pace.slow());
        torch
    }

    #[test]
    fn a_press_at_rest_counts_once_two_64_hz_ticks_after_the_wake_see_it() {
        for (down, lights) in [(2, false), (3, true)] {
            let mut torch = at_rest();
            paced(&mut torch, true, down);
            assert!(!torch.pace.slow());
            paced(&mut torch, false, TICKS_PER_SECOND);
            assert_eq!(torch.level() > 0, lights, "down for {down} ticks");
        }
    }

    #[test]
    fn a_press_at_rest_is_seen_whatever_the_rest_tick_it_lands_by() {
        for wait in 0..2 * TICKS_PER_SECOND {
            let mut torch = at_rest();
            paced(&mut torch, false, wait);
            paced(&mut torch, true, 4);
            paced(&mut torch, false, RELEASE_TICKS + 2);
            assert_eq!(torch.level(), ui::ON_LEVEL, "{wait} ticks on");
        }
    }

    #[test]
    fn the_light_only_comes_on_with_the_watchdogs_window() {
        let mut torch = Rig::default();
        let mut rng = Rng::new(0x5EED);
        let mut lit = 0;
        for _ in 0..40_000 {
            let pressed = rng.next_u16() < 0x5000;
            let span = rng.next_u16() % (2 * TICKS_PER_SECOND);
            for _ in 0..span {
                torch.tick_in(pressed, &Scene::NORMAL);
                if torch.level() > 0 {
                    assert!(torch.pace.windowed());
                    lit += 1;
                }
            }
        }
        assert!(lit > 0);
    }

    #[test]
    fn a_locked_torch_rests() {
        let mut torch = Rig::default();
        lock(&mut torch);
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
        assert!(torch.resting());
        run(&mut torch, false, SHOW_TICKS);
        assert_eq!(torch.aux(), Aux::Off);
        click(&mut torch);
        assert_eq!(torch.level(), 0);
        assert_eq!(torch.aux(), Aux::Yellow);
        run(&mut torch, false, SHOW_TICKS);
        assert_eq!(torch.aux(), Aux::Off);
        run(&mut torch, true, TICKS_PER_SECOND);
        run(&mut torch, false, RELEASE_TICKS + 1);
        clicks(&mut torch, 3);
        run(&mut torch, false, RELEASE_TICKS + 1);
        assert_eq!(torch.level(), 0);
        assert_eq!(torch.aux(), Aux::Yellow, "not locked out");
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
        shows_for(&mut torch, Aux::Yellow, SHOW_TICKS);
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
        assert!(torch.level() > 0 && torch.level() < CEILING);
        assert_eq!(torch.aux(), Aux::Off, "out while the main LED's lit");
        run(&mut torch, false, 3 * TICKS_PER_SECOND);
        assert_eq!(torch.level(), 0);
        assert_eq!(torch.aux(), Aux::White);
        run(&mut torch, false, SHOW_TICKS);
        assert_eq!(torch.aux(), Aux::Off);
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
        until_fault(&mut torch);
        run(&mut torch, true, 4);
        torch.tick(false);
        assert!(torch.level() > 0, "still fading when the click counts");
        assert_eq!(torch.tick(false), Some(Event::Click(1)), "as it's let go");
        assert_eq!(torch.level(), 0);
    }

    #[test]
    fn a_2h_locks_a_fault_thats_fading_though_the_click_puts_it_out() {
        let mut torch = Rig::default();
        at_the_top(&mut torch);
        torch.adc.voltage = 0;
        until_fault(&mut torch);
        run(&mut torch, false, TICKS_PER_SECOND);
        assert!(torch.level() > 0, "still fading");
        two_h(&mut torch);
        assert_eq!(torch.level(), 0);
        assert_eq!(torch.limits.health(), Health::Fault);
        assert_eq!(torch.torch.ui.mode(), Mode::Lockout);
    }

    #[test]
    fn a_2h_locks_a_fault_that_ignores_clicks() {
        let mut torch = Rig::default();
        at_the_top(&mut torch);
        torch.adc.voltage = 0;
        until_fault(&mut torch);
        run(&mut torch, false, TICKS_PER_SECOND);
        click(&mut torch);
        assert_eq!(torch.level(), 0);
        click(&mut torch);
        assert_eq!(torch.torch.ui.mode(), Mode::Off);
        two_h(&mut torch);
        assert_eq!(torch.torch.ui.mode(), Mode::Lockout);
    }

    #[test]
    fn the_aux_says_why_for_three_seconds_to_the_tick() {
        const THREE_SECONDS: u16 = 3 * 64;
        assert_eq!(TICKS_PER_SECOND, 64);
        let mut torch = Rig::new(Faults {
            hang: true,
            ..Faults::NONE
        });
        shows_for(&mut torch, Aux::White, THREE_SECONDS);
        let mut torch = Rig::default();
        click(&mut torch);
        torch.adc.voltage = voltage_reading(280);
        run(&mut torch, false, 20 * TICKS_PER_SECOND);
        assert_eq!(torch.aux(), Aux::Off);
        run(&mut torch, true, 4);
        torch.tick(false);
        assert_eq!(torch.aux(), Aux::Off, "not before the click counts");
        assert_eq!(torch.tick(false), Some(Event::Click(1)));
        shows_for(&mut torch, Aux::Yellow, THREE_SECONDS);
    }

    #[test]
    fn a_fault_that_fades_out_shows_white_for_the_three_seconds_after() {
        let mut torch = Rig::default();
        at_the_top(&mut torch);
        torch.adc.voltage = 0;
        until_fault(&mut torch);
        let mut fade = 0;
        while torch.level() != 0 {
            assert_eq!(torch.aux(), Aux::Off, "out while the main LED's lit");
            torch.tick(false);
            fade += 1;
        }
        assert!(fade > TICKS_PER_SECOND, "a fade of {fade} ticks");
        shows_for(&mut torch, Aux::White, SHOW_TICKS);
    }

    #[test]
    fn a_fault_clicked_off_mid_fade_shows_white_for_the_three_seconds_after() {
        let mut torch = Rig::default();
        at_the_top(&mut torch);
        torch.adc.voltage = 0;
        until_fault(&mut torch);
        run(&mut torch, false, TICKS_PER_SECOND);
        run(&mut torch, true, 4);
        torch.tick(false);
        assert!(torch.level() > 0, "still fading when the click counts");
        assert_eq!(torch.tick(false), Some(Event::Click(1)));
        assert_eq!(torch.level(), 0);
        shows_for(&mut torch, Aux::White, SHOW_TICKS);
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

    #[test]
    fn a_switch_held_from_power_on_lights_the_floor_and_stays_there() {
        let mut torch = Rig::default();
        for tick in 0..10 * 60 * u32::from(TICKS_PER_SECOND) {
            torch.tick(true);
            let lit = tick > u32::from(HOLD_TICKS);
            assert_eq!(torch.level(), if lit { ui::FLOOR } else { 0 }, "{tick}");
        }
    }

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
        for _ in 0..4 {
            run(&mut torch, false, u16::MAX);
        }
        assert_eq!(torch.output(), dim());
        click(&mut torch);
        assert_eq!(torch.output(), Output::OFF);
    }

    fn between(rng: &mut Rng, low: u16, high: u16) -> u16 {
        low + rng.next_u16() % (high - low + 1)
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Pace {
        Quick,
        Slow,
    }

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

    #[derive(Clone, Copy, Debug)]
    enum Source {
        Steady(u16),
        Glitchy(u16),
        Garbage,
    }

    impl Source {
        fn read(self, rng: &mut Rng) -> u16 {
            match self {
                Self::Steady(raw) => raw,
                Self::Glitchy(raw) if rng.next_u16() % 4 != 0 => raw,
                Self::Glitchy(_) | Self::Garbage => match rng.next_u16() % 2 {
                    0 => rng.next_u16(),
                    _ => rng.next_u16() % (FULL_SCALE + 1),
                },
            }
        }

        fn cell(rng: &mut Rng) -> Self {
            match rng.next_u16() % 16 {
                0..=5 => Self::Steady(voltage_reading(between(rng, 340, 420))),
                6..=7 => Self::Steady(voltage_reading(between(rng, 300, 339))),
                8..=9 => Self::Steady(voltage_reading(between(rng, 291, 299))),
                10..=11 => Self::Steady(voltage_reading(between(rng, 150, 289))),
                12 => Self::Steady(match rng.next_u16() % 3 {
                    0 => 0,
                    1 => voltage_reading(60),
                    _ => FULL_SCALE,
                }),
                13..=14 => Self::Glitchy(voltage_reading(between(rng, 300, 420))),
                _ => Self::Garbage,
            }
        }

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

    #[derive(Debug)]
    struct Model {
        mode: Mode,
        asked: u8,
        up: bool,
        dark_at: u32,
        red_until: u32,
        cell_low: bool,
        weak: bool,
        check_in: u16,
        checked_at: u32,
    }

    impl Model {
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
            self.weak = volts < battery::LIGHT_FROM;
        }
    }

    impl Model {
        fn handle(&mut self, event: Event) {
            match (self.mode, event) {
                (Mode::Off | Mode::On, TWO_H) => self.mode = Mode::Lockout,
                (Mode::Off, Event::Click(1)) => {
                    self.mode = Mode::On;
                    self.asked = ON_LEVEL;
                    self.up = false;
                }
                (
                    Mode::Off,
                    Event::Hold {
                        clicks: 0,
                        ticks: 0,
                    },
                ) => {
                    self.mode = Mode::On;
                    self.asked = ui::FLOOR;
                    self.up = false;
                }
                (Mode::Lockout, TWO_H) | (Mode::On, Event::Click(1)) => {
                    self.mode = Mode::Off;
                }
                (Mode::On, Event::Hold { clicks: 0, ticks }) => {
                    self.up ^= ticks == 0;
                    self.asked = if self.up {
                        (self.asked + 1).min(CEILING)
                    } else {
                        (self.asked - 1).max(ui::FLOOR)
                    };
                }
                _ => {}
            }
        }
    }

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
        between_levels: u32,
        failed_at_power_on: u32,
        blue: u32,
        red: u32,
        yellow: u32,
        white: u32,
        results_lost: u32,
        turned_on: u32,
        lit_by_a_hold: u32,
        went_flat: u32,
        refused: u32,
        cut: u32,
        charged: u32,
        went_blind: u32,
        clicked_off_mid_fade: u32,
        ignored_presses: u32,
        blue_dark_for_the_cell: u32,
        darkened_by_a_press: u32,
        checks: u32,
        darkened_at_a_check: u32,
        relit_at_a_check: u32,
        garbage_at_a_check: u32,
        charged_at_a_check: u32,
    }

    #[expect(
        clippy::too_many_lines,
        clippy::similar_names,
        reason = "one walk of the torch beside its model, tick by tick, whose names pair up"
    )]
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
            weak: false,
            check_in: 0,
            checked_at: 0,
        };
        let start_up = pace == Pace::Slow && rng.next_u16().is_multiple_of(2);
        let (mut pressed, mut switch_left) = (start_up, 0);
        let (mut cell, mut head) = (Source::Steady(3516), Source::Steady(2124));
        let (mut lose, mut world_left) = (0, 0);
        let (mut up, mut faulted_for) = (0_u32, 0_u32);
        let (mut converted, mut converted_before) = (None, None);
        let mut lit_before_last = false;
        let (mut asked_before, mut shown_before) = (0, Output::OFF);
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

            let health_before = rig.limits.health();
            let level_before = rig.level();
            let resting_before = rig.resting();
            let mut result = rig.adc.result();
            if lose != 0 && rng.next_u16().is_multiple_of(lose) {
                seen.results_lost += u32::from(result.take().is_some());
            }
            let event = rig.torch.tick(pressed, result, 1);
            rig.adc
                .start_with(rig.torch.convert(), |channel| match channel {
                    Channel::Voltage => cell.read(rng),
                    Channel::Ground => 0,
                    Channel::Temperature => head.read(rng),
                });
            let health = rig.limits.health();
            let level = rig.level();
            let convert = rig.convert();

            let mut ignored = false;
            if let Some(event) = event {
                let press = matches!(event, Event::Click(_) | Event::Hold { ticks: 0, .. });
                if health_before == Health::Good || level_before > 0 {
                    model.handle(event);
                    if press && model.mode == Mode::Lockout {
                        model.red_until = t + u32::from(LOCKED_TICKS);
                    }
                } else {
                    if matches!(model.mode, Mode::Off | Mode::On) && event == TWO_H {
                        model.mode = Mode::Lockout;
                    }
                    if press {
                        model.dark_at = t + u32::from(SHOW_TICKS);
                        ignored = true;
                    }
                }
            }
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

            check!(t, level <= CEILING, "level {level}");
            let shown = rig.output();
            let heat = level == rig.limits.ceilings().1 && asked == asked_before;
            if heat && level > 0 && level_before > 0 {
                let (from, to) = (shown_before.dsm, ramp::output(level).dsm);
                check!(t, shown.enable, "{shown:?}");
                check!(t, shown.dsm >= from.min(to), "{shown:?} at level {level}");
                check!(t, shown.dsm <= from.max(to), "{shown:?} at level {level}");
                seen.between_levels += u32::from(shown != ramp::output(level));
            } else if level == 0 {
                check!(t, !shown.enable, "{shown:?} with the light off");
                let charging = shown.dsm > 0;
                check!(
                    t,
                    !charging || (model.mode == Mode::Off && rig.limits.health() == Health::Good),
                    "{shown:?} in {:?}",
                    model.mode
                );
            } else {
                check!(t, shown == ramp::output(level), "level {level}");
            }
            (asked_before, shown_before) = (asked, shown);
            check!(t, level <= asked, "level {level} of {asked}");
            check!(t, (level > 0) == (model.mode == Mode::On), "level {level}");
            check!(t, !failed || level == 0, "lit after a failed check");
            let moved = level.abs_diff(level_before);
            let click = event == Some(Event::Click(1));
            let locked = event == Some(TWO_H);
            let held = event
                == Some(Event::Hold {
                    clicks: 0,
                    ticks: 0,
                });
            if level_before == 0 && level > 0 {
                check!(t, click || held, "on without a click or a hold: {event:?}");
                check!(t, health_before == Health::Good, "{health_before:?}");
                check!(t, health == Health::Good, "{health:?}");
                let most = if held { ui::FLOOR } else { ON_LEVEL };
                check!(t, level <= most, "on at {level}");
                seen.turned_on += 1;
                seen.lit_by_a_hold += u32::from(held);
            } else if level_before > 0 && level == 0 {
                check!(
                    t,
                    click || locked || health != Health::Good,
                    "went out with nothing wrong and no click or 2H"
                );
            } else {
                check!(t, moved <= 1, "{level_before} to {level}");
            }

            match (health_before, health) {
                (Health::Good, Health::Good)
                | (Health::Flat, Health::Flat)
                | (Health::Fault, Health::Fault) => {}
                (Health::Good, Health::Flat) => {
                    let volts = result.map(battery::centivolts);
                    match level_before {
                        0 => {
                            check!(t, click || held, "refused without a press: {event:?}");
                            check!(t, model.weak, "refused a cell it never read as weak");
                            seen.refused += 1;
                        }
                        1 => {}
                        _ => {
                            let cut = volts.is_some_and(|v| v < battery::CUT_BELOW);
                            check!(t, cut, "flat from level {level_before} at {volts:?}");
                            seen.cut += 1;
                        }
                    }
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
                check!(t, level <= level_before, "{level_before} to {level}");
                if level > 0 {
                    check!(t, moved == 1, "{level_before} to {level}");
                } else if level_before > 0 {
                    check!(
                        t,
                        click || locked || level_before == 1,
                        "out from {level_before}"
                    );
                    seen.clicked_off_mid_fade += u32::from(click);
                }
            }

            let aux = rig.aux();
            check!(t, level == 0 || aux == Aux::Off, "{aux:?} at level {level}");
            let showing = t < model.dark_at;
            let expected = match (health, model.mode) {
                _ if level > 0 => Aux::Off,
                (Health::Good, Mode::Off) if !model.cell_low => Aux::Blue,
                (Health::Good, Mode::Lockout) if t < model.red_until || showing => Aux::Red,
                (Health::Flat, _) if showing => Aux::Yellow,
                (Health::Fault, _) if showing => Aux::White,
                _ => Aux::Off,
            };
            check!(t, aux == expected, "{aux:?}, {health:?}, {model:?}");

            let resting = rig.resting();
            if resting {
                check!(t, level == 0, "resting at level {level}");
                check!(t, rig.convert().is_none(), "resting with the ADC busy");
            }
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
            model.check_in = if resting {
                model.check_in.saturating_sub(1)
            } else {
                CHECK_TICKS
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

    #[expect(
        clippy::similar_names,
        reason = "the seed it starts from and what it has seen"
    )]
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
            ("between two levels", seen.between_levels),
            ("failed at power-on", seen.failed_at_power_on),
            ("blue", seen.blue),
            ("red", seen.red),
            ("yellow", seen.yellow),
            ("white", seen.white),
            ("missing a result", seen.results_lost),
            ("turned on", seen.turned_on),
            ("lit by a hold", seen.lit_by_a_hold),
            ("gone flat", seen.went_flat),
            ("refused to light a weak cell", seen.refused),
            ("cut while lit", seen.cut),
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
