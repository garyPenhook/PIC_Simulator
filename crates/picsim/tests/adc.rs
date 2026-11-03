//! ADCC basic mode and the `[[adc]]` stimulus (DS40002043F ch.33, PIC18F27Q10).

use devdesc::Device;
use picsim::Machine;
use std::path::Path;

fn machine(stim: &str) -> Machine {
    let dev = Device::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../devices/18F27Q10.toml")).unwrap();
    let mut m = Machine::new(dev).unwrap();
    m.load_stimulus(stim).unwrap();
    m
}

fn addr(m: &Machine, n: &str) -> u32 {
    m.dev.find_sfr(n).unwrap_or_else(|| panic!("no {n}")).addr
}

fn adif(m: &mut Machine) -> bool {
    let s = m.dev.sfr.iter().find(|s| s.fields.iter().any(|f| f.name == "ADIF")).unwrap();
    let f = s.fields.iter().find(|f| f.name == "ADIF").unwrap();
    m.cpu.read(s.addr) & (1 << f.pos) != 0
}

fn run_to(m: &mut Machine, c: u64) {
    while m.cpu.cycles < c {
        m.cpu.step();
    }
}

#[test]
fn conversion_time_result_and_flag() {
    let mut m = machine("[[adc]]\nchannel = 13\nvalue = 0x2A5\n");
    let (con, clk, acq, pch) = (addr(&m, "ADCON0"), addr(&m, "ADCLK"), addr(&m, "ADACQ"), addr(&m, "ADPCH"));
    let (rl, rh) = (addr(&m, "ADRESL"), addr(&m, "ADRESH"));
    m.cpu.write(clk, 3); // TAD = 8 Fosc periods = 2 cycles
    m.cpu.write(acq, 2);
    m.cpu.write(pch, 13);
    m.cpu.write(con, 0x84); // ADON, ADFM
    let t0 = m.cpu.cycles;
    m.cpu.write(con, 0x85);
    // (ADACQ 2 + 11.5) TAD = 13.5 * 2 cycles = 27 cycles
    run_to(&mut m, t0 + 26);
    assert_eq!(m.cpu.read(con) & 1, 1, "still converting");
    assert!(!adif(&mut m));
    run_to(&mut m, t0 + 28);
    assert_eq!(m.cpu.read(con) & 1, 0, "ADGO cleared");
    assert!(adif(&mut m));
    assert_eq!((m.cpu.read(rh), m.cpu.read(rl)), (0x02, 0xA5));
}

#[test]
fn left_justified_and_unknown_channel() {
    let mut m = machine("[[adc]]\nchannel = 1\nvalue = 0x3FF\n");
    let (con, pch) = (addr(&m, "ADCON0"), addr(&m, "ADPCH"));
    let (rl, rh) = (addr(&m, "ADRESL"), addr(&m, "ADRESH"));
    m.cpu.write(pch, 1);
    m.cpu.write(con, 0x80);
    m.cpu.write(con, 0x81);
    run_to(&mut m, 1000);
    assert_eq!((m.cpu.read(rh), m.cpu.read(rl)), (0xFF, 0xC0));
    m.cpu.write(pch, 2); // nothing driven there
    m.cpu.write(con, 0x81);
    run_to(&mut m, 2000);
    assert_eq!((m.cpu.read(rh), m.cpu.read(rl)), (0, 0));
}

#[test]
fn stimulus_changes_over_time_and_adon_off_terminates() {
    let mut m = machine("[[adc]]\nchannel = 0\nvalue = 100\n[[adc]]\nchannel = 0\nvalue = 200\nat = 5000\n");
    let (con, rl) = (addr(&m, "ADCON0"), addr(&m, "ADRESL"));
    m.cpu.write(con, 0x84);
    m.cpu.write(con, 0x85);
    run_to(&mut m, 1000);
    assert_eq!(m.cpu.read(rl), 100);
    run_to(&mut m, 6000);
    m.cpu.write(con, 0x85);
    m.cpu.write(con, 0x04); // ADON off in the middle: terminated, ADRES keeps its value
    run_to(&mut m, 7000);
    assert_eq!(m.cpu.read(rl), 100);
    assert_eq!(m.cpu.read(con) & 1, 0);
    m.cpu.write(con, 0x05); // ADON is off: ADGO does not stick
    assert_eq!(m.cpu.read(con) & 1, 0);
    m.cpu.write(con, 0x84);
    m.cpu.write(con, 0x85);
    run_to(&mut m, 8000);
    assert_eq!(m.cpu.read(rl), 200);
}

#[test]
fn continuous_mode_retriggers_until_adgo_cleared() {
    let mut m = machine("[[adc]]\nchannel = 0\nvalue = 7\n");
    let con = addr(&m, "ADCON0");
    m.cpu.write(con, 0xC4); // ADON, ADCONT, ADFM
    m.cpu.write(con, 0xC5);
    run_to(&mut m, 2000);
    assert_eq!(m.cpu.read(con) & 1, 1, "ADGO stays set with ADCONT");
    m.cpu.write(con, 0xC4);
    run_to(&mut m, 3000);
    assert_eq!(m.cpu.read(con) & 1, 0);
}

#[test]
fn fvr_ready_follows_enable() {
    let mut m = machine("");
    let f = addr(&m, "FVRCON");
    m.cpu.write(f, 0x81);
    assert_eq!(m.cpu.read(f) & 0x40, 0x40);
    m.cpu.write(f, 0xFF); // FVRRDY is read only; it stays what the hardware says
    assert_eq!(m.cpu.read(f) & 0x40, 0x40);
    m.cpu.write(f, 0x01);
    assert_eq!(m.cpu.read(f) & 0x40, 0);
}

#[test]
fn bad_adc_stimulus_is_rejected() {
    let dev = Device::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../devices/18F27Q10.toml")).unwrap();
    let mut m = Machine::new(dev).unwrap();
    assert!(m.load_stimulus("[[adc]]\nchannel = 64\nvalue = 1\n").is_err());
    assert!(m.load_stimulus("[[adc]]\nchannel = 1\n").is_err());
    assert!(m.load_stimulus("[[adc]]\nchannel = 1\nvalue = 1\nbogus = 2\n").is_err());
}

// ---- Q41 (DS40002214G ch.40, PIC18F16Q41): 12 bits, ADPRE/ADACQ 13 bits in Fosc clocks, 14 TAD + 2 TCY ----

fn machine_q41(stim: &str) -> Machine {
    let dev = Device::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../devices/18F16Q41.toml")).unwrap();
    let mut m = Machine::new(dev).unwrap();
    m.load_stimulus(stim).unwrap();
    m
}

#[test]
fn q41_conversion_time_result_and_justification() {
    let mut m = machine_q41("[[adc]]\nchannel = 22\nvalue = 0xABC\n");
    let (con, clk, acql, acqh, pch) = (addr(&m, "ADCON0"), addr(&m, "ADCLK"), addr(&m, "ADACQL"), addr(&m, "ADACQH"), addr(&m, "ADPCH"));
    let (rl, rh) = (addr(&m, "ADRESL"), addr(&m, "ADRESH"));
    m.cpu.write(clk, 3); // TAD = 8 Fosc = 2 cycles
    m.cpu.write(acql, 16); // 16 Fosc = 4 cycles
    m.cpu.write(pch, 22);
    m.cpu.write(con, 0x84); // ON, FM = 1
    let t0 = m.cpu.cycles;
    m.cpu.write(con, 0x85);
    // 4 + 14 * 2 + 2 = 34 cycles
    run_to(&mut m, t0 + 33);
    assert_eq!(m.cpu.read(con) & 1, 1, "still converting");
    assert!(!adif(&mut m));
    run_to(&mut m, t0 + 35);
    assert_eq!(m.cpu.read(con) & 1, 0, "GO cleared");
    assert!(adif(&mut m));
    assert_eq!((m.cpu.read(rh), m.cpu.read(rl)), (0x0A, 0xBC));
    // left justified (FM = 0): the 12-bit code shifted up four places
    m.cpu.write(con, 0x80);
    m.cpu.write(con, 0x81);
    let t1 = m.cpu.cycles;
    run_to(&mut m, t1 + 40);
    assert_eq!((m.cpu.read(rh), m.cpu.read(rl)), (0xAB, 0xC0));
    // 13-bit acquisition: ACQH = 1 -> 256 Fosc = 64 cycles more
    m.cpu.write(acqh, 1);
    m.cpu.write(acql, 0);
    m.cpu.write(con, 0x81);
    let t2 = m.cpu.cycles;
    run_to(&mut m, t2 + 64 + 29);
    assert_eq!(m.cpu.read(con) & 1, 1, "still converting");
    run_to(&mut m, t2 + 64 + 31);
    assert_eq!(m.cpu.read(con) & 1, 0);
}

#[test]
fn q41_acq_zero_with_precharge_is_8192_clocks() {
    let mut m = machine_q41("[[adc]]\nchannel = 0\nvalue = 5\n");
    let (con, clk, prel) = (addr(&m, "ADCON0"), addr(&m, "ADCLK"), addr(&m, "ADPREL"));
    m.cpu.write(clk, 0); // TAD = 2 Fosc
    m.cpu.write(prel, 4); // 4 + 8192 Fosc = 2049 cycles, then 7 + 2
    m.cpu.write(con, 0x84);
    let t0 = m.cpu.cycles;
    m.cpu.write(con, 0x85);
    run_to(&mut m, t0 + 2049 + 8);
    assert_eq!(m.cpu.read(con) & 1, 1);
    run_to(&mut m, t0 + 2049 + 11);
    assert_eq!(m.cpu.read(con) & 1, 0);
    assert_eq!(m.cpu.read(addr(&m, "ADRESL")), 5); // FM = 1: right justified
}

#[test]
fn q41_go_is_not_set_by_the_instruction_that_turns_the_adc_on() {
    let mut m = machine_q41("[[adc]]\nchannel = 0\nvalue = 9\n");
    let con = addr(&m, "ADCON0");
    m.cpu.write(con, 0x85); // ON and GO together
    assert_eq!(m.cpu.read(con) & 1, 0, "GO ignored");
    run_to(&mut m, 500);
    assert!(!adif(&mut m));
    m.cpu.write(con, 0x85); // ON already set
    assert_eq!(m.cpu.read(con) & 1, 1);
}
