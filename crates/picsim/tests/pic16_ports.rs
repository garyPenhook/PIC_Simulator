//! Ports and PPS on the PIC16 core: PORTx / LATx / TRISx / ANSELx, the `[[pin]]` / `[[port]]` stimulus, the PPS input
//! decoding and PPSLOCK with PPS1WAY. PIC16F17146 (DS40002343F ch.16, ch.18) and PIC16F18346 (DS40001839E ch.12, ch.13).

use devdesc::Device;
use picsim::machine16::Machine16;
use std::path::Path;

fn movlw(k: u16) -> u16 { 0x3000 | k }
fn movwf(f: u16) -> u16 { 0x0080 | f }
fn movf_w(f: u16) -> u16 { 0x0800 | f }
fn bsf(f: u16, b: u16) -> u16 { 0x1400 | b << 7 | f }
fn bcf(f: u16, b: u16) -> u16 { 0x1000 | b << 7 | f }
/// MOVLB: the PIC16F183xx has 5-bit banks and its own encoding (DS40001839E p.406); the other parts 6 bits.
fn movlb(part: &str, k: u16) -> u16 { if part == "16F18346" { 0x0020 | k } else { 0x0140 | k } }
const NOP: u16 = 0;
const RESET: u16 = 0x0001;

fn hexrec(b: &[u8]) -> String {
    let sum = b.iter().fold(0u8, |a, &x| a.wrapping_add(x));
    format!(":{}{:02X}\n", b.iter().map(|x| format!("{x:02X}")).collect::<String>(), sum.wrapping_neg())
}

/// `part` with `prog` at word 0. `one_way` is the PPS1WAY configuration bit (CONFIG2 bit 11 on both parts; the
/// unprogrammed value is 1).
fn machine(part: &str, prog: &[u16], one_way: bool, stim: Option<&str>) -> Machine16 {
    build(part, prog, one_way, true, stim)
}

/// The same with the MCLRE configuration bit chosen too (every other configuration bit is unprogrammed).
fn build(part: &str, prog: &[u16], one_way: bool, mclre: bool, stim: Option<&str>) -> Machine16 {
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
    for r in m.dev.config.clone() {
        let mut v = r.default;
        for f in &r.fields {
            if (f.name == "PPS1WAY" && !one_way) || (f.name == "MCLRE" && !mclre) {
                v &= !(((1u32 << f.width) - 1) << f.pos);
            }
        }
        put(r.addr, v as u16);
    }
    m.load_hex(&(text + ":00000001FF\n")).unwrap();
    if let Some(s) = stim {
        m.load_stimulus(s).unwrap();
    }
    m
}

fn addr(m: &Machine16, n: &str) -> u16 {
    m.dev.find_sfr(n).unwrap_or_else(|| panic!("no {n}")).addr as u16
}

fn step(m: &mut Machine16, n: usize) {
    for _ in 0..n {
        m.cpu.step();
    }
}

fn run_to(m: &mut Machine16, c: u64) {
    while m.cpu.cycles < c {
        m.cpu.step();
    }
}

/// Instructions that write `v` to the register at `a` (any bank).
fn wr(part: &str, a: u16, v: u16) -> Vec<u16> {
    vec![movlb(part, a >> 7), movlw(v), movwf(a & 0x7F)]
}

/// The PPS lock (`set`) or unlock sequence of the data sheets: MOVLW 55h, MOVWF, MOVLW AAh, MOVWF, BSF / BCF.
fn lock_seq(part: &str, a: u16, set: bool) -> Vec<u16> {
    let (b, f) = (a >> 7, a & 0x7F);
    vec![movlb(part, b), movlw(0x55), movwf(f), movlw(0xAA), movwf(f), if set { bsf(f, 0) } else { bcf(f, 0) }]
}

fn cat(parts: &[Vec<u16>]) -> Vec<u16> {
    parts.concat()
}

const PARTS: [&str; 2] = ["16F17146", "16F18346"];

#[test]
fn pins_follow_the_stimulus_and_analog_pins_read_zero() {
    for part in PARTS {
        let stim = "[[port]]\nport = \"C\"\nvalue = 0xFF\nat = 0\n[[pin]]\nport = \"C\"\nbit = 2\nlevel = 0\nat = 60\n";
        let probe = machine(part, &[], true, None);
        let (portc, ansc) = (addr(&probe, "PORTC"), addr(&probe, "ANSELC"));
        let prog = cat(&[
            vec![movlb(part, portc >> 7), movf_w(portc & 0x7F), movwf(0x20)], // ANSELC resets to all ones: analog, reads 0
            wr(part, ansc, 0),
            vec![movlb(part, portc >> 7), movf_w(portc & 0x7F), movwf(0x21)],
            vec![NOP; 60],
            vec![movf_w(portc & 0x7F), movwf(0x22)], // after cycle 60 RC2 is driven low
        ]);
        let mut m = machine(part, &prog, true, Some(stim));
        step(&mut m, 3 + 3 + 3);
        assert_eq!(m.cpu.mem[0x20], 0, "{part}: analog pins read 0");
        let full = m.cpu.mem[0x21];
        assert_ne!(full, 0, "{part}: digital input pins show the stimulus");
        assert_eq!(full & !m.cpu.implemented[usize::from(portc)], 0, "{part}: only the pins that exist");
        step(&mut m, 60 + 2);
        assert!(m.cpu.cycles > 60);
        assert_eq!(m.cpu.mem[0x22], full & !4, "{part}: RC2 driven low from cycle 60");
    }
}

#[test]
fn output_pins_show_the_latch_and_a_port_write_goes_to_the_latch() {
    for part in PARTS {
        let probe = machine(part, &[], true, None);
        let (porta, lata, trisa, ansa) = (addr(&probe, "PORTA"), addr(&probe, "LATA"), addr(&probe, "TRISA"), addr(&probe, "ANSELA"));
        let mask = probe.cpu.implemented[usize::from(porta)];
        let stim = "[[port]]\nport = \"A\"\nvalue = 0xFF\n";
        // RA1:0 outputs, the rest inputs driven high
        let prog = cat(&[
            wr(part, ansa, 0),
            wr(part, trisa, 0xFC),
            wr(part, lata, 0x01),
            vec![movlb(part, 0), movf_w(porta & 0x7F), movwf(0x20)],
            vec![movlw(0x02), movwf(porta & 0x7F)], // a write to PORTA is a write to LATA
            vec![movf_w(porta & 0x7F), movwf(0x21)],
        ]);
        let mut m = machine(part, &prog, true, Some(stim));
        step(&mut m, prog.len());
        assert_eq!(m.cpu.mem[0x20], (0xFC | 0x01) & mask, "{part}: inputs from the stimulus, RA0 from LATA");
        assert_eq!(m.cpu.mem[usize::from(lata)], 0x02, "{part}: LATA got the PORTA write");
        assert_eq!(m.cpu.mem[0x21], (0xFC | 0x02) & mask, "{part}: PORTA after the write");
        assert_eq!(m.cpu.peek(u32::from(porta)), m.cpu.mem[0x21], "{part}: peek shows the pins");
    }
}

#[test]
fn read_modify_write_on_a_port_uses_the_pins_not_the_latch() {
    // BSF PORTA,1 reads the pins: RA2 is an input driven high, so it ends in LATA although the latch had it clear
    // (DS40002343F 16.3 p.180: "the PORT pins are read, and this value is modified and then written to the PORT data latch")
    let part = "16F17146";
    let probe = machine(part, &[], true, None);
    let (porta, lata, trisa, ansa) = (addr(&probe, "PORTA"), addr(&probe, "LATA"), addr(&probe, "TRISA"), addr(&probe, "ANSELA"));
    let prog = cat(&[wr(part, ansa, 0), wr(part, trisa, 0xFE), vec![movlb(part, 0), bsf(porta & 0x7F, 1)]]);
    let mut m = machine(part, &prog, true, Some("[[pin]]\nport = \"A\"\nbit = 2\nlevel = 1\n"));
    step(&mut m, prog.len());
    assert_eq!(m.cpu.mem[usize::from(lata)], 0x06);
}

#[test]
fn pps_input_register_selects_a_pin() {
    for part in PARTS {
        let probe = machine(part, &[], true, None);
        let (intpps, ansc) = (addr(&probe, "INTPPS"), addr(&probe, "ANSELC"));
        // PORT field 2 = C, PIN 3: RC3 in both layouts (6 bits with PORT[2:0], 5 bits with the PIC16F183xx's 10011)
        let prog = cat(&[wr(part, ansc, 0), wr(part, intpps, 0x13), vec![NOP; 60]]); // NOPs: erased flash is not one-cycle code
        let mut m = machine(part, &prog, true, Some("[[pin]]\nport = \"C\"\nbit = 3\nlevel = 1\nat = 30\n"));
        step(&mut m, 6);
        run_to(&mut m, 29);
        assert_eq!(m.cpu.pps_input(usize::from(intpps)), Some(false), "{part}");
        run_to(&mut m, 31);
        assert_eq!(m.cpu.pps_input(usize::from(intpps)), Some(true), "{part}");
        m.cpu.mem[usize::from(intpps)] = 0x3F; // PORT 7 is no port of the part
        assert_eq!(m.cpu.pps_input(usize::from(intpps)), None, "{part}");
    }
}

#[test]
fn pps_lock_needs_the_sequence_and_blocks_pps_writes() {
    for part in PARTS {
        let probe = machine(part, &[], false, None);
        let (lock, rc0) = (addr(&probe, "PPSLOCK"), addr(&probe, "RC0PPS"));
        let (lock_a, rc0_a) = (usize::from(lock), usize::from(rc0));
        let prog = cat(&[
            wr(part, rc0, 0x04), // unlocked by default: the write works
            wr(part, lock, 0x01), // PPSLOCKED written without the sequence: ignored
            lock_seq(part, lock, true),
            wr(part, rc0, 0x07), // locked: ignored
            wr(part, lock, 0x00), // locked, no sequence: stays locked
            lock_seq(part, lock, false),
            wr(part, rc0, 0x07), // unlocked again
        ]);
        let mut m = machine(part, &prog, false, None);
        let marks = [3usize, 3, 6, 3, 3, 6, 3];
        let mut done = 0;
        let mut seen = Vec::new();
        for n in marks {
            done += n;
            step(&mut m, n);
            seen.push((m.cpu.mem[rc0_a], m.cpu.mem[lock_a]));
            assert!(done <= prog.len());
        }
        assert_eq!(seen, vec![(4, 0), (4, 0), (4, 1), (4, 1), (4, 1), (4, 0), (7, 0)], "{part}");
    }
}

#[test]
fn a_broken_sequence_does_not_unlock() {
    for part in PARTS {
        let probe = machine(part, &[], false, None);
        let lock = addr(&probe, "PPSLOCK");
        let f = lock & 0x7F;
        let prog = cat(&[
            lock_seq(part, lock, true),
            // 55h, then a different value, then AAh and BCF: the first step was lost
            vec![movlw(0x55), movwf(f), movlw(0x12), movwf(f), movlw(0xAA), movwf(f), bcf(f, 0)],
            // 55h, AAh, then BCF of a second register's bit position (still PPSLOCK, but 55h was not repeated)
            lock_seq(part, lock, false),
        ]);
        let mut m = machine(part, &prog, false, None);
        step(&mut m, 6);
        assert_eq!(m.cpu.mem[usize::from(lock)], 1, "{part}");
        step(&mut m, 7);
        assert_eq!(m.cpu.mem[usize::from(lock)], 1, "{part}: still locked after a broken sequence");
        step(&mut m, 6);
        assert_eq!(m.cpu.mem[usize::from(lock)], 0, "{part}: the full sequence unlocks");
    }
}

#[test]
fn pps1way_allows_one_lock_and_no_unlock_until_a_reset() {
    for part in PARTS {
        let probe = machine(part, &[], true, None);
        let (lock, rc0) = (addr(&probe, "PPSLOCK"), addr(&probe, "RC0PPS"));
        let prog = cat(&[lock_seq(part, lock, true), lock_seq(part, lock, false), wr(part, rc0, 0x03), vec![RESET], lock_seq(part, lock, true)]);
        let mut m = machine(part, &prog, true, None);
        step(&mut m, 6);
        assert_eq!(m.cpu.mem[usize::from(lock)], 1, "{part}");
        step(&mut m, 6);
        assert_eq!(m.cpu.mem[usize::from(lock)], 1, "{part}: PPS1WAY: it cannot be cleared again");
        step(&mut m, 3);
        assert_eq!(m.cpu.mem[usize::from(rc0)], 0, "{part}: RC0PPS write ignored while locked");
        step(&mut m, 1);
        assert_eq!(m.cpu.mem[usize::from(lock)], 0, "{part}: PPSLOCKED is cleared by the Reset");
        assert_eq!(m.cpu.pc, 0, "{part}");
        step(&mut m, 6);
        assert_eq!(m.cpu.mem[usize::from(lock)], 1, "{part}: and can be set once more");
    }
}

#[test]
fn a_ppsout_source_other_than_lat_is_noted() {
    let probe = machine("16F17146", &[], true, None);
    let rc0 = addr(&probe, "RC0PPS");
    let mut m = machine("16F17146", &wr("16F17146", rc0, 0x13), true, None);
    step(&mut m, 3);
    assert!(m.cpu.notes.iter().any(|n| n.contains("RxyPPS")), "{:?}", m.cpu.notes);
}

#[test]
fn the_mclr_pin_reads_one_while_enabled_and_is_a_digital_input_otherwise() {
    // RA3 reads 1 when MCLRE = 1 (DS40002343F 16.13 p.182, DS40002195E p.130, DS40002519D p.168, DS40001839E Register 12-1
    // note 2); with MCLRE = 0 it is a digital input-only pin that shows what drives it, and ANSELA does not affect it
    for part in ["16F17146", "16F18346", "16F13145", "16F15244"] {
        let probe = machine(part, &[], true, None);
        let porta = addr(&probe, "PORTA");
        let prog = [movlb(part, porta >> 7), movf_w(porta & 0x7F), movwf(0x20)];
        let stim = "[[pin]]\nport = \"A\"\nbit = 3\nlevel = 0\n";
        for (mclre, want) in [(true, 0x08), (false, 0x00)] {
            let mut m = build(part, &prog, true, mclre, Some(stim));
            step(&mut m, 3);
            assert_eq!(m.cpu.mem[0x20] & 0x08, want, "{part} MCLRE={mclre}");
        }
        let mut m = build(part, &prog, true, false, Some("[[pin]]\nport = \"A\"\nbit = 3\nlevel = 1\n"));
        step(&mut m, 3);
        assert_eq!(m.cpu.mem[0x20] & 0x08, 0x08, "{part}: driven high with MCLRE = 0, ANSELA3 left at its reset value");
    }
}

#[test]
fn the_tris_bit_of_the_mclr_pin_is_read_only_one_where_it_exists() {
    // PIC16F152xx: "The TRIS bit associated with the MCLR pin is read-only and the value is '1'" (DS40002195E 16.14.3 p.133);
    // PIC16F183xx: TRISA3 is "U-1" (DS40001839E Register 12-2 p.143). The PIC16F171xx and PIC16F131xx have no such bit.
    for (part, bit3) in [("16F15244", 8), ("16F18346", 8), ("16F17146", 0), ("16F13145", 0)] {
        let probe = machine(part, &[], true, None);
        let trisa = addr(&probe, "TRISA");
        let prog = cat(&[wr(part, trisa, 0x00), vec![movf_w(trisa & 0x7F), movwf(0x70)]]);
        let mut m = machine(part, &prog, true, None);
        step(&mut m, prog.len());
        assert_eq!(m.cpu.mem[0x70] & 0x08, bit3, "{part}");
    }
}
