//! The watchdog on the PIC16 core: the windowed WWDT of the PIC16F131xx and PIC16F171xx (DS40002343F ch.14 p.144-153, Table 10-4
//! p.76; PIC16F13145 DS40002519D) and the single-register WDT of the PIC16F152xx and PIC16F183xx (DS40002195E ch.14 p.103-106,
//! DS40001839E ch.10 p.120-121). Fosc is 64 MHz (the default), so one 31 kHz watchdog clock is 516.13 instruction cycles:
//! PS = 0 times out after 32 clocks = 16517 cycles.

use devdesc::Device;
use picsim::machine16::Machine16;
use std::path::Path;

const WINDOWED: [&str; 3] = ["16F17146", "16F13145", "16F13245"];
const SINGLE: [&str; 3] = ["16F15244", "16F18346", "16F18056"];
const NOP: u16 = 0x0000;
const SLEEP: u16 = 0x0063;
const CLRWDT: u16 = 0x0064;
const BRA_SELF: u16 = 0x33FF;
const GOTO_0: u16 = 0x2800;
const TIME_OUT: u64 = 16517;

fn hexrec(b: &[u8]) -> String {
    let sum = b.iter().fold(0u8, |a, &x| a.wrapping_add(x));
    format!(":{}{:02X}\n", b.iter().map(|x| format!("{x:02X}")).collect::<String>(), sum.wrapping_neg())
}

/// `part` with `prog` at word 0 and the configuration fields in `cfg` changed (every other field is unprogrammed).
fn machine(part: &str, prog: &[u16], cfg: &[(&str, u32)]) -> Machine16 {
    let dev = Device::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("../../devices/{part}.toml"))).unwrap();
    let mut m = Machine16::new(dev).unwrap();
    let mut text = String::new();
    let mut put = |w: u32, v: u16| {
        for (a, b) in [(2 * w, v as u8), (2 * w + 1, (v >> 8) as u8)] {
            text += &hexrec(&[2, 0, 0, 4, (a >> 24) as u8, (a >> 16) as u8]);
            text += &hexrec(&[1, (a >> 8) as u8, a as u8, 0, b]);
        }
    };
    for (i, &w) in prog.iter().enumerate() {
        put(i as u32, w);
    }
    let mut set = vec![false; cfg.len()];
    for r in m.dev.config.clone() {
        let mut v = r.default;
        for f in &r.fields {
            if let Some(k) = cfg.iter().position(|c| c.0 == f.name) {
                let mask = ((1u32 << f.width) - 1) << f.pos;
                v = v & !mask | (cfg[k].1 << f.pos) & mask;
                set[k] = true;
            }
        }
        put(r.addr, v as u16);
    }
    assert!(set.iter().all(|&s| s), "{part}: a configuration field of {cfg:?} does not exist");
    m.load_hex(&(text + ":00000001FF\n")).unwrap();
    m
}

fn a(m: &Machine16, n: &str) -> u32 {
    m.dev.find_sfr(n).unwrap_or_else(|| panic!("no {n}")).addr
}

fn r(m: &mut Machine16, n: &str) -> u8 {
    let x = a(m, n);
    m.cpu.read(x)
}

fn w(m: &mut Machine16, n: &str, v: u8) {
    let x = a(m, n);
    m.cpu.write(x, v);
}

fn con0(part: &str) -> &'static str {
    if WINDOWED.contains(&part) { "WDTCON0" } else { "WDTCON" }
}

fn run_to(m: &mut Machine16, c: u64) {
    while m.cpu.cycles < c {
        m.cpu.step();
    }
}

fn rwdt(m: &mut Machine16) -> bool {
    r(m, "PCON0") & 0x10 == 0
}

/// Run until the part resets (RWDT or WDTWV cleared in PCON0) or `limit` cycles have passed; the cycle of the reset.
fn until_reset(m: &mut Machine16, limit: u64) -> Option<u64> {
    // WDTWV (bit 5) exists on the windowed WWDT only
    let mask = if m.dev.device.name.contains("17146") || m.dev.device.name.contains("13145") { 0x30 } else { 0x10 };
    while m.cpu.cycles < limit {
        m.cpu.step();
        if r(m, "PCON0") & mask != mask {
            return Some(m.cpu.cycles);
        }
    }
    None
}

#[test]
fn reset_values_follow_the_configuration() {
    for part in WINDOWED {
        let mut m = machine(part, &[BRA_SELF], &[("WDTE", 3)]);
        assert_eq!(r(&mut m, "WDTCON0"), 0x16, "{part}: PS = 01011 (2 s), SEN = 0");
        assert_eq!(r(&mut m, "WDTCON1"), 0x07, "{part}: no window, LFINTOSC");
        let mut m = machine(part, &[BRA_SELF], &[("WDTE", 3), ("WDTCPS", 5), ("WDTCWS", 2), ("WDTCCS", 1)]);
        assert_eq!(r(&mut m, "WDTCON0"), 0x0A, "{part}: PS = WDTCPS");
        assert_eq!(r(&mut m, "WDTCON1"), 0x12, "{part}: CS = WDTCCS, WINDOW = WDTCWS");
        w(&mut m, "WDTCON0", 0x00);
        w(&mut m, "WDTCON1", 0x00);
        assert_eq!(r(&mut m, "WDTCON0"), 0x0A, "{part}: PS is read only when WDTCPS is set");
        assert_eq!(r(&mut m, "WDTCON1"), 0x12, "{part}: CS and WINDOW are read only when the configuration sets them");
    }
    for part in SINGLE {
        let mut m = machine(part, &[BRA_SELF], &[("WDTE", 3)]);
        assert_eq!(r(&mut m, "WDTCON"), 0x16, "{part}: PS = 01011 (2 s), SEN = 0");
    }
}

#[test]
fn time_out_resets_the_part_and_clears_to_and_rwdt() {
    for part in WINDOWED.into_iter().chain(SINGLE) {
        let mut m = machine(part, &[NOP, BRA_SELF], &[("WDTE", 3)]);
        w(&mut m, con0(part), 0x00); // PS = 0: 32 clocks (and the write clears the watchdog at cycle 0)
        assert!(!rwdt(&mut m), "{part}: RWDT is set at power-on");
        let c = until_reset(&mut m, 3 * TIME_OUT).unwrap_or_else(|| panic!("{part}: no time-out"));
        assert!((TIME_OUT..TIME_OUT + 5).contains(&c), "{part}: reset at cycle {c}");
        assert_eq!(r(&mut m, "STATUS") & 0x18, 0x08, "{part}: TO = 0, PD unchanged (Table 10-4)");
        assert_eq!(m.cpu.pc, 0, "{part}");
        assert_eq!(r(&mut m, con0(part)), 0x16, "{part}: the registers are back at their reset values");
    }
}

#[test]
fn clrwdt_keeps_the_part_alive() {
    for part in WINDOWED.into_iter().chain(SINGLE) {
        let mut m = machine(part, &[CLRWDT, GOTO_0], &[("WDTE", 3)]);
        w(&mut m, con0(part), 0x00);
        assert_eq!(until_reset(&mut m, 5 * TIME_OUT), None, "{part}");
        assert_eq!(r(&mut m, "STATUS") & 0x18, 0x18, "{part}: CLRWDT sets TO and PD");
    }
}

#[test]
fn off_and_software_enabled_modes() {
    for part in WINDOWED.into_iter().chain(SINGLE) {
        let c0 = con0(part);
        // WDTE = 0: never
        let mut m = machine(part, &[NOP, BRA_SELF], &[("WDTE", 0)]);
        w(&mut m, c0, 0x01); // SEN is ignored
        w(&mut m, c0, 0x01);
        assert_eq!(until_reset(&mut m, 3 * TIME_OUT), None, "{part}: WDTE = 0");
        // WDTE = 1: SEN decides
        let mut m = machine(part, &[NOP, BRA_SELF], &[("WDTE", 1)]);
        w(&mut m, c0, 0x00);
        assert_eq!(until_reset(&mut m, 3 * TIME_OUT), None, "{part}: SEN = 0");
        w(&mut m, c0, 0x01); // PS = 0, SEN = 1
        let start = m.cpu.cycles;
        let c = until_reset(&mut m, start + 3 * TIME_OUT).unwrap_or_else(|| panic!("{part}: SEN = 1 gives no time-out"));
        assert!((start + TIME_OUT..start + TIME_OUT + 5).contains(&c), "{part}: reset at {c}, SEN set at {start}");
    }
}

#[test]
fn a_time_out_in_sleep_wakes_the_part_without_a_reset() {
    for part in WINDOWED.into_iter().chain(SINGLE) {
        let c0 = con0(part);
        let mut m = machine(part, &[SLEEP, NOP, BRA_SELF], &[("WDTE", 3)]);
        w(&mut m, c0, 0x00);
        m.cpu.step(); // SLEEP: the watchdog is cleared
        let t0 = m.cpu.cycles;
        assert!(!m.cpu.sleep_dead(), "{part}: the watchdog will wake the part");
        while m.cpu.pc == 1 && m.cpu.cycles < t0 + 3 * TIME_OUT {
            m.cpu.step();
        }
        let slept = m.cpu.cycles - t0;
        assert!((TIME_OUT - 2..TIME_OUT + 4).contains(&slept), "{part}: woke after {slept} cycles");
        assert_eq!(m.cpu.pc, 2, "{part}: execution goes on after SLEEP, no reset");
        assert_eq!(r(&mut m, "STATUS") & 0x18, 0x00, "{part}: TO = PD = 0");
        // RWDT: the single-register WDT clears it (DS40002195E 14.5), the windowed one leaves PCON0 alone (Table 10-4)
        assert_eq!(rwdt(&mut m), SINGLE.contains(&part), "{part}: RWDT");
    }
}

#[test]
fn the_watchdog_that_is_off_in_sleep_leaves_the_part_asleep() {
    for part in WINDOWED.into_iter().chain(SINGLE) {
        let mut m = machine(part, &[SLEEP, BRA_SELF], &[("WDTE", 2)]);
        m.cpu.step();
        assert!(m.cpu.sleep_dead(), "{part}: WDTE = 2 does not count in Sleep");
        let mut m = machine(part, &[SLEEP, BRA_SELF], &[("WDTE", 1)]);
        m.cpu.step();
        assert!(m.cpu.sleep_dead(), "{part}: WDTE = 1 with SEN = 0");
        let mut m = machine(part, &[SLEEP, BRA_SELF], &[("WDTE", 1)]);
        w(&mut m, con0(part), 0x01);
        m.cpu.step();
        assert!(!m.cpu.sleep_dead(), "{part}: WDTE = 1 with SEN = 1");
    }
}

#[test]
fn a_sleep_with_the_default_two_second_period_takes_one_step_per_jump() {
    // 65536 clocks of 516 cycles are 33.8 million cycles: far too many to step one by one, so the sleep jumps to the time-out
    let mut m = machine("16F17146", &[SLEEP, NOP, BRA_SELF], &[("WDTE", 3)]);
    let mut steps = 0;
    while m.cpu.pc < 2 && steps < 1000 {
        m.cpu.step();
        steps += 1;
    }
    assert_eq!(m.cpu.pc, 2);
    assert!((33_800_000..33_900_000).contains(&m.cpu.cycles), "{}", m.cpu.cycles);
}

#[test]
fn a_clrwdt_outside_the_window_is_a_window_violation() {
    // WDTCWS = 3: WINDOW = 011, the window opens at 50 % of the period (DS40002343F 14.7.2 p.149). PS = 0: 16517 cycles.
    for part in WINDOWED {
        // not armed: any CLRWDT is a violation, however late (14.5.1)
        let mut m = machine(part, &[NOP, CLRWDT, BRA_SELF], &[("WDTE", 3), ("WDTCWS", 3)]);
        w(&mut m, "WDTCON0", 0x00);
        m.cpu.step();
        m.cpu.step();
        assert_eq!(r(&mut m, "PCON0") & 0x30, 0x10, "{part}: WDTWV = 0, RWDT = 1");
        assert_eq!(m.cpu.pc, 0, "{part}");
        assert_eq!(r(&mut m, "STATUS") & 0x18, 0x18, "{part}: a violation does not change TO (`[A91]`)");

        // armed, but the window is closed: a violation
        let mut m = machine(part, &[NOP, CLRWDT, BRA_SELF], &[("WDTE", 3), ("WDTCWS", 3)]);
        w(&mut m, "WDTCON0", 0x00);
        r(&mut m, "WDTCON0"); // arms
        m.cpu.step();
        m.cpu.step();
        assert_eq!(r(&mut m, "PCON0") & 0x30, 0x10, "{part}: closed window");

        // armed with the window open: the watchdog is cleared and the part goes on
        let mut m = machine(part, &[CLRWDT, GOTO_0], &[("WDTE", 3), ("WDTCWS", 3)]);
        w(&mut m, "WDTCON0", 0x00);
        r(&mut m, "WDTCON0");
        m.cpu.cycles = 9000; // past 50 % of the period (the window opened at 8259)
        // the watchdog started at cycle 0
        m.cpu.step();
        assert_eq!(r(&mut m, "PCON0") & 0x30, 0x30, "{part}: valid CLRWDT");
        assert_eq!(r(&mut m, "WDTTMR") >> 3, 0, "{part}: the window counter starts again");
    }
}

#[test]
fn the_registers_show_the_count() {
    for part in WINDOWED {
        let mut m = machine(part, &[BRA_SELF], &[("WDTE", 3)]);
        w(&mut m, "WDTCON0", 0x00); // 32 clocks
        run_to(&mut m, 10 * 516);
        let t = r(&mut m, "WDTTMR");
        assert_eq!(t >> 3, 9, "{part}: TMR[4:0] counts clocks in 32nds of the period (n = 0), {t:#04x}");
        assert_eq!(t & 4, 0, "{part}: not armed");
        r(&mut m, "WDTCON0");
        assert_eq!(r(&mut m, "WDTTMR") & 4, 4, "{part}: STATE shows the WWDT armed");
    }
}
