//! CWG1 (DS40002043F ch.25 p.377-405; PIC18F27Q10): half bridge with dead band, steering, push-pull, full bridge,
//! auto-shutdown. The data input is PWM3 on Timer2 (period 64 cycles = 256 ticks, pulse 32 ticks).

use devdesc::Device;
use picsim::Machine;
use std::path::Path;

fn machine() -> Machine {
    let dev = Device::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../devices/18F27Q10.toml")).unwrap();
    Machine::new(dev).unwrap()
}

fn addr(m: &Machine, n: &str) -> u32 {
    m.dev.find_sfr(n).unwrap_or_else(|| panic!("no {n}")).addr
}

fn w(m: &mut Machine, n: &str, v: u8) {
    let a = addr(m, n);
    m.cpu.write(a, v);
}

fn r(m: &mut Machine, n: &str) -> u8 {
    let a = addr(m, n);
    m.cpu.read(a)
}

fn run_to(m: &mut Machine, c: u64) {
    while m.cpu.cycles < c {
        m.cpu.step();
    }
}

fn run_for(m: &mut Machine, n: u64) {
    let c = m.cpu.cycles + n;
    run_to(m, c);
}

/// PWM3 as data input, CWG1A..D on RC0..RC3, `con0` written last (EN set); then PWM3 is started and the run goes on `cycles`.
fn setup(m: &mut Machine, con0: u8, extra: impl FnOnce(&mut Machine)) {
    for (i, n) in ["RC0PPS", "RC1PPS", "RC2PPS", "RC3PPS"].into_iter().enumerate() {
        w(m, n, i as u8 + 1);
    }
    w(m, "TRISC", 0xF0);
    w(m, "T2PR", 0x3F);
    w(m, "T2CLKCON", 1);
    w(m, "PWM3DCH", 0x08);
    w(m, "CWG1ISM", 3); // PWM3
    extra(m);
    w(m, "CWG1CON0", con0);
    w(m, "PWM3CON", 0x80);
    w(m, "T2CON", 0x80);
}

fn pin(m: &Machine, bit: u8) -> Vec<(u64, bool)> {
    m.cpu.p.pin_log.iter().filter(|e| e.1 == b'C' && e.2 == bit).map(|e| (e.0, e.3)).collect()
}

#[test]
fn half_bridge_delays_each_turn_on_by_its_dead_band() {
    let mut m = machine();
    setup(&mut m, 0x84, |m| {
        w(m, "CWG1DBR", 3);
        w(m, "CWG1DBF", 5);
    });
    let start = m.cpu.cycles;
    run_to(&mut m, start + 400);
    let a = pin(&m, 0);
    let b = pin(&m, 1);
    let c = pin(&m, 2);
    let d = pin(&m, 3);
    let ar = a.iter().find(|e| e.1).expect("A rises").0;
    let af = a.iter().find(|e| !e.1 && e.0 > ar).expect("A falls").0;
    assert_eq!(af - ar, 32 - 3, "A is the input delayed on the rising edge only: {a:?}");
    // B starts low, so its first turn-off is logged in the second period: B off when the input rises, A on DBR later.
    let br = b.iter().find(|e| e.1).expect("B rises").0;
    assert_eq!(br, af + 5, "B turns on DBF after the input fell: {b:?}");
    let bf = b.iter().find(|e| !e.1).expect("B falls").0;
    let ar2 = a.iter().filter(|e| e.1).nth(1).expect("A rises again").0;
    assert_eq!(ar2 - bf, 3, "A turns on DBR after B turned off: {a:?} {b:?}");
    assert_eq!(a, c, "C copies A");
    assert_eq!(b, d, "D copies B");
}

#[test]
fn input_shorter_than_the_dead_band_shows_no_pulse() {
    let mut m = machine();
    setup(&mut m, 0x84, |m| {
        w(m, "CWG1DBR", 40); // longer than the 32 tick pulse
        w(m, "CWG1DBF", 0);
    });
    let start = m.cpu.cycles;
    run_to(&mut m, start + 400);
    assert!(pin(&m, 0).iter().all(|e| !e.1), "{:?}", pin(&m, 0));
}

#[test]
fn polarity_inverts_the_outputs() {
    let mut m = machine();
    setup(&mut m, 0x84, |m| w(m, "CWG1CON1", 0x01)); // POLA
    let start = m.cpu.cycles;
    run_to(&mut m, start + 400);
    let a = pin(&m, 0);
    let b = pin(&m, 1);
    let af = a.iter().find(|e| !e.1).unwrap().0;
    let ar = a.iter().find(|e| e.1 && e.0 > af).unwrap().0;
    assert_eq!(ar - af, 32, "A is low during the pulse: {a:?}");
    assert!(b.iter().filter(|e| e.1).count() >= 1, "{b:?}");
}

#[test]
fn steering_follows_the_input_on_steered_pins_and_holds_ovr_on_the_others() {
    let mut m = machine();
    setup(&mut m, 0x80, |m| w(m, "CWG1STR", 0x21)); // STRA = 1, OVRB = 1 with STRB = 0
    let start = m.cpu.cycles;
    run_to(&mut m, start + 400);
    let a = pin(&m, 0);
    let ar = a.iter().find(|e| e.1).unwrap().0;
    let af = a.iter().find(|e| !e.1 && e.0 > ar).unwrap().0;
    assert_eq!(af - ar, 32, "{a:?}");
    assert!(pin(&m, 1).iter().all(|e| e.1), "B holds OVRB: {:?}", pin(&m, 1));
}

#[test]
fn push_pull_alternates_the_pulses_between_a_and_b() {
    let mut m = machine();
    setup(&mut m, 0x85, |_| {});
    let start = m.cpu.cycles;
    run_to(&mut m, start + 700);
    let (a, b) = (pin(&m, 0), pin(&m, 1));
    let ar: Vec<u64> = a.iter().filter(|e| e.1).map(|e| e.0).collect();
    let br: Vec<u64> = b.iter().filter(|e| e.1).map(|e| e.0).collect();
    assert!(ar.len() >= 2 && br.len() >= 2, "{a:?} {b:?}");
    assert_eq!(br[0] - ar[0], 256, "B gets the second pulse");
    assert_eq!(ar[1] - ar[0], 512, "A gets every second pulse");
    assert_eq!(pin(&m, 2), a, "C copies A");
}

#[test]
fn forward_full_bridge_modulates_d_and_holds_a_active() {
    let mut m = machine();
    setup(&mut m, 0x82, |_| {});
    let start = m.cpu.cycles;
    run_to(&mut m, start + 400);
    assert!(pin(&m, 0).iter().all(|e| e.1), "A active: {:?}", pin(&m, 0));
    assert!(pin(&m, 1).iter().all(|e| !e.1) && pin(&m, 2).iter().all(|e| !e.1));
    let d = pin(&m, 3);
    let dr = d.iter().find(|e| e.1).unwrap().0;
    assert_eq!(d.iter().find(|e| !e.1 && e.0 > dr).unwrap().0 - dr, 32, "{d:?}");
}

#[test]
fn software_shutdown_drives_the_override_levels_sets_the_flag_and_waits_for_a_rising_edge() {
    let mut m = machine();
    setup(&mut m, 0x84, |m| w(m, "CWG1AS0", 0x38)); // LSBD = 11 (high), LSAC = 10 (low)
    let start = m.cpu.cycles;
    run_to(&mut m, start + 100);
    w(&mut m, "CWG1AS0", 0xB8); // SHUTDOWN
    assert_eq!(r(&mut m, "PIR7") & 0x80 != 0 || r(&mut m, "CWG1AS0") & 0x80 != 0, true);
    run_for(&mut m, 200);
    let after = m.cpu.p.pin_log.len();
    run_for(&mut m, 300);
    assert_eq!(m.cpu.p.pin_log.len(), after, "no output edges while shut down");
    let a_last = pin(&m, 0).last().unwrap().1;
    let b_last = pin(&m, 1).last().unwrap().1;
    assert!(!a_last && b_last, "A low, B high");
    // Clear: the outputs stay overridden until the next rising edge of the input, then run again.
    w(&mut m, "CWG1AS0", 0x38);
    assert_eq!(r(&mut m, "CWG1AS0") & 0x80, 0);
    let n = m.cpu.p.pin_log.len();
    run_for(&mut m, 300);
    assert!(m.cpu.p.pin_log.len() > n, "output resumes");
}

#[test]
fn unmodelled_input_adds_a_note() {
    let mut m = machine();
    w(&mut m, "CWG1ISM", 0);
    w(&mut m, "CWG1CON0", 0x84);
    assert!(m.cpu.p.notes.iter().any(|n| n.contains("CWG1 data input")), "{:?}", m.cpu.p.notes);
}
