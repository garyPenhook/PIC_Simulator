//! The ADC on the PIC16 core (PIC16F17146, DS40002343F ch.33 p.485-536): the ADCC of the Q41 (`periph/adc.rs`).
use devdesc::Device;
use picsim::machine16::Machine16;
use std::path::Path;

fn movlw(k: u16) -> u16 { 0x3000 | k }
fn movwf(f: u16) -> u16 { 0x0080 | f }
fn movlb(k: u16) -> u16 { 0x0140 | k }
const NOP: u16 = 0;

const A_PCH: usize = 0x1D1F;
const A_NCH: usize = 0x1D20;
const A_ACQL: usize = 0x1D21;
const A_PREL: usize = 0x1D24;
const A_CLK: usize = 0x1D2D;
const A_FVR: usize = 0x020C;
const A_PIE6: usize = 0x9C;
const A_CON0: usize = 0x1D26;
const A_RESL: usize = 0x1D1D;
const A_RESH: usize = 0x1D1E;
const PIR6: usize = 0x92;

fn hexrec(b: &[u8]) -> String {
    let sum = b.iter().fold(0u8, |a, &x| a.wrapping_add(x));
    format!(":{}{:02X}\n", b.iter().map(|x| format!("{x:02X}")).collect::<String>(), sum.wrapping_neg())
}

fn machine(prog: &[u16], isr: &[u16], stim: &str) -> Machine16 {
    machine_of("16F17146", prog, isr, stim)
}

fn machine_of(part: &str, prog: &[u16], isr: &[u16], stim: &str) -> Machine16 {
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
    for (i, &w) in isr.iter().enumerate() {
        put(4 + i as u32, w);
    }
    put(0x8008, 0x3FFF);
    m.load_hex(&(text + ":00000001FF\n")).unwrap();
    if !stim.is_empty() {
        m.load_stimulus(stim).unwrap();
    }
    m
}

/// Each triple loads one register of the part: absolute address, value.
fn set(regs: &[(usize, u16)]) -> Vec<u16> {
    set_for(false, regs)
}

/// The PIC16F183xx has a 5-bit BSR and its own MOVLB encoding (0020h-003Fh).
fn set_for(bsr5: bool, regs: &[(usize, u16)]) -> Vec<u16> {
    let mut p = Vec::new();
    for &(r, k) in regs {
        let b = (r >> 7) as u16;
        p.extend([if bsr5 { 0x0020 | b } else { movlb(b) }, movlw(k), movwf((r & 0x7F) as u16)]);
    }
    p
}

fn run(m: &mut Machine16, n: u64) {
    let end = m.cpu.cycles + n;
    while m.cpu.cycles < end {
        m.cpu.step();
    }
}

/// Run until ADCON0's GO bit is set; returns the cycle count after the instruction that set it.
fn until_go(m: &mut Machine16) -> u64 {
    for _ in 0..1000 {
        m.cpu.step();
        if m.cpu.mem[A_CON0] & 1 != 0 {
            return m.cpu.cycles;
        }
    }
    panic!("GO never set");
}

fn res(m: &Machine16) -> u16 {
    u16::from(m.cpu.mem[A_RESH]) << 8 | u16::from(m.cpu.mem[A_RESL])
}

fn go(m: &Machine16) -> bool {
    m.cpu.mem[A_CON0] & 1 != 0
}

fn adif(m: &Machine16) -> bool {
    m.cpu.mem[PIR6] & 1 != 0
}

/// Program: configure, turn the ADC on with `on`, start with `on | 1`, then NOPs.
fn convert(regs: &[(usize, u16)], on: u16) -> Vec<u16> {
    let mut p = set(regs);
    p.extend(set(&[(A_CON0, on), (A_CON0, on | 1)]));
    p.extend([NOP; 4000]);
    p
}

#[test]
fn power_on_state() {
    let m = machine(&[NOP], &[], "");
    // ADCON0 resets to 02h: IC = 1, the differential mode (33.5.1 p.509)
    assert_eq!(m.cpu.mem[A_CON0], 0x02);
}

#[test]
fn conversion_time_result_flag_and_justification() {
    let prog = convert(&[(A_CLK, 3), (A_ACQL, 16), (A_PCH, 22)], 0x84);
    let mut m = machine(&prog, &[], "[[adc]]\nchannel = 22\nvalue = 0xABC\n");
    let t0 = until_go(&mut m);
    // acquisition 16 Fosc = 4 cycles, 14 TAD of 8 Fosc = 28 cycles, 2 TCY: 34 cycles (AD21 p.635)
    run(&mut m, 32);
    assert!(go(&m) && !adif(&m), "still converting");
    while m.cpu.cycles < t0 + 34 {
        m.cpu.step();
    }
    assert!(!go(&m), "GO cleared");
    assert!(adif(&m));
    assert_eq!(m.cpu.cycles, t0 + 34);
    assert_eq!(res(&m), 0x0ABC, "FM = 01: right justified");
}

#[test]
fn left_justified_single_ended_and_unknown_channel() {
    let prog = convert(&[(A_CLK, 0), (A_PCH, 5)], 0x80);
    let mut m = machine(&prog, &[], "[[adc]]\nchannel = 5\nvalue = 0xABC\n");
    until_go(&mut m);
    run(&mut m, 30);
    assert!(!go(&m));
    assert_eq!(res(&m), 0xABC0, "FM = 00, IC = 0: the code shifted up four places");
    let prog = convert(&[(A_CLK, 0), (A_PCH, 6)], 0x84);
    let mut m = machine(&prog, &[], "[[adc]]\nchannel = 5\nvalue = 0xABC\n");
    until_go(&mut m);
    run(&mut m, 30);
    assert_eq!(res(&m), 0, "nothing is driven on channel 6");
    assert!(adif(&m));
}

#[test]
fn acq_zero_with_precharge_is_8192_clocks() {
    let prog = convert(&[(A_CLK, 0), (A_PREL, 4), (A_PCH, 0)], 0x84);
    let mut m = machine(&prog, &[], "[[adc]]\nchannel = 0\nvalue = 5\n");
    let t0 = until_go(&mut m);
    // (4 + 8192) / 4 = 2049 cycles, then 14 * 2 / 4 = 7 and 2
    run(&mut m, 2049 + 7);
    assert!(go(&m));
    while m.cpu.cycles < t0 + 2049 + 9 {
        m.cpu.step();
    }
    assert!(!go(&m));
    assert_eq!(res(&m), 5);
}

#[test]
fn go_is_not_set_by_the_instruction_that_turns_the_adc_on() {
    let mut p = set(&[(A_CON0, 0x85)]);
    p.extend([NOP; 300]);
    let mut m = machine(&p, &[], "[[adc]]\nchannel = 0\nvalue = 9\n");
    run(&mut m, 200);
    assert!(!go(&m), "ON and GO together: GO ignored (33.2.1 p.494)");
    assert!(!adif(&m));
}

#[test]
fn continuous_mode_retriggers_until_go_is_cleared() {
    let mut p = convert(&[(A_CLK, 0), (A_PCH, 0)], 0xC4);
    p.truncate(p.len() - 4000);
    p.extend([NOP; 450]);
    p.extend(set(&[(A_CON0, 0xC4)])); // GO cleared by software: stops
    p.extend([NOP; 100]);
    let mut m = machine(&p, &[], "[[adc]]\nchannel = 0\nvalue = 7\n");
    until_go(&mut m);
    run(&mut m, 400);
    assert!(go(&m), "GO stays set with CONT");
    assert!(adif(&m));
    run(&mut m, 100);
    assert!(!go(&m));
}

#[test]
fn turning_the_adc_off_terminates_and_adres_keeps_its_value() {
    let mut p = convert(&[(A_CLK, 0), (A_PCH, 0)], 0x84);
    p.truncate(p.len() - 4000);
    p.extend([NOP; 40]);
    p.extend(set(&[(A_CON0, 0x04), (A_CON0, 0x05)])); // ON off: stops; GO with ON = 0 sticks to nothing
    p.extend([NOP; 100]);
    let mut m = machine(&p, &[], "[[adc]]\nchannel = 0\nvalue = 100\n[[adc]]\nchannel = 0\nvalue = 200\nat = 30\n");
    until_go(&mut m);
    run(&mut m, 60);
    assert_eq!(res(&m), 100, "the code at the end of acquisition (cycle 20)");
    assert!(adif(&m));
    run(&mut m, 200);
    assert!(!go(&m));
}

#[test]
fn differential_mode_formats() {
    // channel 1 = 3000, channel 2 = 1000: +2000 with ADPCH = 1, -2000 with the channels swapped
    let stim = "[[adc]]\nchannel = 1\nvalue = 3000\n[[adc]]\nchannel = 2\nvalue = 1000\n";
    for (con, pos, want) in [
        (0x82u16 | 0x04, 2u16, 0xF830u16), // FM = 01, IC = 1: right justified, two's complement
        (0x82, 2, 0x8300),                   // FM = 00: left justified, two's complement
        (0x82 | 0x08, 2, 0xFD00),            // FM = 10: left justified, sign/magnitude
        (0x82 | 0x0C, 2, 0xFFD0),            // FM = 11: right justified, sign/magnitude
        (0x82 | 0x04, 1, 0x07D0),
        (0x82, 1, 0x7D00),
        (0x82 | 0x08, 1, 0x7D00),
        (0x82 | 0x0C, 1, 0x07D0),
    ] {
        let neg = 3 - pos;
        let prog = convert(&[(A_CLK, 0), (A_PCH, pos), (A_NCH, neg)], con);
        let mut m = machine(&prog, &[], stim);
        until_go(&mut m);
        run(&mut m, 30);
        assert!(!go(&m));
        assert_eq!(res(&m), want, "ADCON0 = {con:02X}, ADPCH = {pos}");
    }
}

#[test]
fn differential_result_is_limited() {
    let prog = convert(&[(A_CLK, 0), (A_PCH, 1), (A_NCH, 2)], 0x86);
    let mut m = machine(&prog, &[], "[[adc]]\nchannel = 2\nvalue = 4095\n");
    until_go(&mut m);
    run(&mut m, 30);
    assert_eq!(res(&m), 0xF801, "0 - 4095 limited to -2047 (Equation 33-4 p.493), right justified two's complement");
    let prog = convert(&[(A_CLK, 0), (A_PCH, 1), (A_NCH, 2)], 0x8E);
    let mut m = machine(&prog, &[], "[[adc]]\nchannel = 2\nvalue = 4095\n");
    until_go(&mut m);
    run(&mut m, 30);
    assert_eq!(res(&m), 0xFFFF_u16 & 0xF800 | 0x07FF, "sign/magnitude: -2047 is the sign and 2047");
    let prog = convert(&[(A_CLK, 0), (A_PCH, 1), (A_NCH, 2)], 0x86);
    let mut m = machine(&prog, &[], "[[adc]]\nchannel = 1\nvalue = 4095\n");
    until_go(&mut m);
    run(&mut m, 30);
    assert_eq!(res(&m), 0x07FE, "4095 - 0 limited to 2046");
}

#[test]
fn interrupt_on_completion() {
    let mut p = set(&[(A_CLK, 0), (A_PCH, 0), (A_PIE6, 1), (0x0B, 0xC0), (A_CON0, 0x84), (A_CON0, 0x85)]);
    p.extend([0x2800 + p.len() as u16]); // GOTO self
    let isr = [NOP];
    let mut m = machine(&p, &isr, "[[adc]]\nchannel = 0\nvalue = 1\n");
    until_go(&mut m);
    let mut vector = false;
    for _ in 0..40 {
        m.cpu.step();
        vector |= m.cpu.pc == 4;
    }
    assert!(vector, "ADIF with ADIE, PEIE and GIE enters the interrupt vector");
    assert!(adif(&m));
}

#[test]
fn fvr_ready_follows_enable() {
    let mut p = set(&[(A_FVR, 0x81)]);
    p.extend([NOP; 4]);
    p.extend(set(&[(A_FVR, 0xFF)])); // FVRRDY is read only
    p.extend(set(&[(A_FVR, 0x41)])); // RDY written 1 with EN = 0
    let mut m = machine(&p, &[], "");
    run(&mut m, 4);
    assert_eq!(m.cpu.mem[A_FVR] & 0x40, 0x40, "FVREN set: FVRRDY at once");
    run(&mut m, 6);
    assert_eq!(m.cpu.mem[A_FVR], 0xFF & !0x00, "FVRRDY is read only: 0xFF written, RDY follows EN");
    run(&mut m, 3);
    assert_eq!(m.cpu.mem[A_FVR] & 0x40, 0, "FVREN cleared");
}

#[test]
fn adc_without_a_model_rejects_the_stimulus() {
    // PIC16F18056/76: DS40002325C is not in the document set, so there is no ADC model
    let dev = Device::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../devices/16F18056.toml")).unwrap();
    let mut m = Machine16::new(dev).unwrap();
    assert!(m.load_stimulus("[[adc]]\nchannel = 0\nvalue = 1\n").is_err());
    let dev = Device::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../devices/16F17146.toml")).unwrap();
    let mut m = Machine16::new(dev).unwrap();
    assert!(m.load_stimulus("[[spi]]\nbytes = [1]\n").is_err());
}

#[test]
fn pic16f131xx_and_132xx_are_10_bit_converters_without_differential_mode() {
    // PIC16F13145: DS40002519D Figure 32-3 p.443: 10 bits, left justified they fill ADRES bits 15..6; AD21 p.557: 14 TAD + 2 TCY.
    // PIC16F132xx: DS40002686B Figure 32-3 p.486 and AD21 p.611: 10 bits, 12 TAD + 2 TCY
    for (part, cycles) in [("16F13145", 9), ("16F13245", 8), ("16F13214", 8), ("16F13224", 8), ("16F13244", 8)] {
        let dev = Device::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("../../devices/{part}.toml"))).unwrap();
        let a = |n: &str| dev.find_sfr(n).unwrap_or_else(|| panic!("no {n}")).addr as usize;
        let (con, clk, pch, rl, rh) = (a("ADCON0"), a("ADCLK"), a("ADPCH"), a("ADRESL"), a("ADRESH"));
        let pir = dev.sfr.iter().find(|s| s.fields.iter().any(|f| f.name == "ADIF")).unwrap();
        let (pir, bit) = (pir.addr as usize, pir.fields.iter().find(|f| f.name == "ADIF").unwrap().pos);
        for (on, want) in [(0x84u16, 0x03FFu16), (0x80, 0xFFC0)] {
            let mut p = set(&[(clk, 0), (pch, 4), (con, on), (con, on | 1)]);
            p.extend([NOP; 100]);
            let mut m = machine_of(part, &p, &[], "[[adc]]\nchannel = 4\nvalue = 0xFFF\n");
            assert_eq!(m.cpu.mem[con], 0, "no IC bit: ADCON0 resets to 0");
            for _ in 0..1000 {
                m.cpu.step();
                if m.cpu.mem[con] & 1 != 0 {
                    break;
                }
            }
            let t0 = m.cpu.cycles;
            while m.cpu.mem[con] & 1 != 0 {
                m.cpu.step();
            }
            assert_eq!(m.cpu.cycles - t0, cycles, "{part}: TAD of 2 Fosc, 2 TCY");
            assert_eq!(u16::from(m.cpu.mem[rh]) << 8 | u16::from(m.cpu.mem[rl]), want, "{part} ADCON0 = {on:02X}: the code is limited to 10 bits");
            assert_eq!(m.cpu.mem[pir] >> bit & 1, 1);
        }
    }
}

#[test]
fn a_part_without_a_sourced_adc_says_so() {
    let mut m = machine_of("16F18056", &[NOP], &[], "");
    m.cpu.step();
    assert!(m.cpu.notes.iter().any(|n| n.contains("ADC of this part is not modelled")), "{:?}", m.cpu.notes);
    assert!(!m.cpu.has_adc());
}

// ---- the simple 10-bit ADC: PIC16F152xx (DS40002195E ch.27) and PIC16F183xx (DS40001839E ch.22) ----

struct Simple {
    part: &'static str,
    con: usize,
    con1: usize,
    rl: usize,
    rh: usize,
    pir: usize,
    bit: u32,
    bsr5: bool,
}

fn simple(part: &'static str) -> Simple {
    let dev = Device::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("../../devices/{part}.toml"))).unwrap();
    let a = |n: &str| dev.find_sfr(n).unwrap_or_else(|| panic!("no {n}")).addr as usize;
    let pir = dev.sfr.iter().find(|s| s.fields.iter().any(|f| f.name == "ADIF")).unwrap();
    Simple {
        part, con: a("ADCON0"), con1: a("ADCON1"), rl: a("ADRESL"), rh: a("ADRESH"), pir: pir.addr as usize,
        bit: u32::from(pir.fields.iter().find(|f| f.name == "ADIF").unwrap().pos), bsr5: part.contains("183"),
    }
}

/// CHS = `chs`, ADCON1 = `con1`, then ON, then ON | GO (bit 1), then NOPs.
fn simple_prog(s: &Simple, chs: u16, con1: u16) -> Vec<u16> {
    let mut p = set_for(s.bsr5, &[(s.con1, con1), (s.con, chs << 2 | 1), (s.con, chs << 2 | 3)]);
    p.extend([NOP; 300]);
    p
}

fn simple_run(s: &Simple, con1: u16) -> (Machine16, u64) {
    let mut m = machine_of(s.part, &simple_prog(s, 5, con1), &[], "[[adc]]\nchannel = 5\nvalue = 0xFFF\n[[adc]]\nchannel = 4\nvalue = 1\n");
    for _ in 0..1000 {
        m.cpu.step();
        if m.cpu.mem[s.con] & 2 != 0 {
            break;
        }
    }
    let t0 = m.cpu.cycles;
    assert!(m.cpu.mem[s.con] & 2 != 0, "GO set");
    assert_eq!(m.cpu.mem[s.pir] >> s.bit & 1, 0);
    while m.cpu.mem[s.con] & 2 != 0 {
        m.cpu.step();
    }
    let n = m.cpu.cycles - t0;
    (m, n)
}

#[test]
fn simple_adc_conversion_time_result_and_flag() {
    // 11.5 TAD `[A113]`, MDB: tests/probe/adc_simple_n.S
    for part in ["16F15244", "16F18346", "16F15213", "16F18313"] {
        let s = simple(part);
        // ADCS = 100: FOSC/4 = one TAD per instruction cycle; FM = 1 right justified; the 12-bit code is limited to 10 bits
        let (m, n) = simple_run(&s, 0x80 | 0x40);
        assert_eq!(n, 12, "{part}: 11.5 TAD of one cycle");
        assert_eq!(u16::from(m.cpu.mem[s.rh]) << 8 | u16::from(m.cpu.mem[s.rl]), 0x03FF, "{part}");
        assert_eq!(m.cpu.mem[s.pir] >> s.bit & 1, 1, "{part}: ADIF");
        // ADCS = 000: FOSC/2, two Fosc periods per TAD; FM = 0 left justified
        let (m, n) = simple_run(&s, 0x00);
        assert_eq!(n, 6, "{part}: 11.5 TAD of half a cycle");
        assert_eq!(u16::from(m.cpu.mem[s.rh]) << 8 | u16::from(m.cpu.mem[s.rl]), 0xFFC0, "{part}");
        // ADCS = 110: FOSC/64
        let (_, n) = simple_run(&s, 0x80 | 0x60);
        assert_eq!(n, 184, "{part}: 11.5 TAD of 16 cycles");
    }
}

#[test]
fn simple_adc_go_needs_the_adc_on_first_and_adcrc_is_noted() {
    let s = simple("16F18346");
    let mut p = set_for(s.bsr5, &[(s.con, 0x15 << 2 | 3)]); // ON and GO together
    p.extend([NOP; 100]);
    let mut m = machine_of(s.part, &p, &[], "");
    run(&mut m, 60);
    assert_eq!(m.cpu.mem[s.con] & 2, 0, "GO is not set by the instruction that turns the ADC on (22.2.1 p.240)");
    assert_eq!(m.cpu.mem[s.pir] >> s.bit & 1, 0);
    let mut p = set_for(s.bsr5, &[(s.con1, 0x30), (s.con, 1), (s.con, 3)]); // ADCS = 011: ADCRC
    p.extend([NOP; 100]);
    let mut m = machine_of(s.part, &p, &[], "");
    run(&mut m, 60);
    assert!(m.cpu.mem[s.con] & 2 != 0, "ADCRC is not modelled: GO stays set");
    assert!(m.cpu.notes.iter().any(|n| n.contains("not modelled")));
}

#[test]
fn simple_adc_sleep_aborts_the_conversion() {
    let s = simple("16F15244");
    let mut p = set(&[(s.con1, 0xF0 & 0x80 | 0x60), (s.con, 5 << 2 | 1), (s.con, 5 << 2 | 3)]);
    p.push(0x0063); // SLEEP
    p.extend([NOP; 50]);
    let mut m = machine_of(s.part, &p, &[], "[[adc]]\nchannel = 5\nvalue = 77\n");
    for _ in 0..200 {
        m.cpu.step();
    }
    assert_eq!(m.cpu.mem[s.con] & 2, 0, "the conversion was aborted: GO clear");
    assert_eq!(m.cpu.mem[s.pir] >> s.bit & 1, 0, "no ADIF");
    assert_eq!((m.cpu.mem[s.rh], m.cpu.mem[s.rl]), (0, 0), "ADRES not loaded");
}


#[test]
fn fvr_ready_is_always_set_on_the_pic16f152xx() {
    // DS40002195E 26.3.1 p.316 note 2; MDB agrees (tests/probe/fvr_rdy.S)
    let s = simple("16F15244");
    let dev = Device::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../devices/16F15244.toml")).unwrap();
    let fvr = dev.find_sfr("FVRCON").unwrap().addr as usize;
    let mut p = set(&[(fvr, 0x00)]);
    p.extend([NOP; 4]);
    let mut m = machine_of(s.part, &p, &[], "");
    assert_eq!(m.cpu.mem[fvr] & 0x40, 0x40, "FVRRDY is 1 at power-on");
    run(&mut m, 6);
    assert_eq!(m.cpu.mem[fvr] & 0xC0, 0x40, "FVREN cleared, FVRRDY still 1");
}
