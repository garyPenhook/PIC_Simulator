//! The data EEPROM of the PIC16F171xx (PIC16F17146, DS40002343F 15.2 p.167-178) and PIC16F183xx (PIC16F18346,
//! DS40001839E ch.11 p.123-139): `periph/nvm16.rs`.
use devdesc::Device;
use picsim::machine16::Machine16;
use std::path::Path;

fn movlw(k: u16) -> u16 { 0x3000 | k }
fn movwf(f: u16) -> u16 { 0x0080 | f }
fn bsf(f: u16, b: u16) -> u16 { 0x1400 | b << 7 | f }
const NOP: u16 = 0;
const RESET: u16 = 1;

/// (NVMADRL, NVMADRH, NVMDATL, NVMCON1, NVMCON2, PIR with NVMIF, its bit, bsr5)
type Map = (usize, usize, usize, usize, usize, usize, u8, bool);
const P171: Map = (0x1C8C, 0x1C8D, 0x1C8E, 0x1C90, 0x1C91, 0x8D, 0x01, false);
const P183: Map = (0x891, 0x892, 0x893, 0x895, 0x896, 0x12, 0x10, true);

fn hexrec(b: &[u8]) -> String {
    let sum = b.iter().fold(0u8, |a, &x| a.wrapping_add(x));
    format!(":{}{:02X}\n", b.iter().map(|x| format!("{x:02X}")).collect::<String>(), sum.wrapping_neg())
}

fn machine(part: &str, prog: &[u16]) -> Machine16 {
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
    // EEPROM byte 7 = 5Ah in the image (word F007h)
    text += &hexrec(&[2, 0, 0, 4, 0, 1]);
    text += &hexrec(&[1, 0xE0, 0x0E, 0, 0x5A]);
    m.load_hex(&(text + ":00000001FF\n")).unwrap();
    m
}

fn set(map: Map, regs: &[(usize, u16)]) -> Vec<u16> {
    let mut p = Vec::new();
    for &(r, k) in regs {
        let b = (r >> 7) as u16;
        p.extend([if map.7 { 0x0020 | b } else { 0x0140 | b }, movlw(k), movwf((r & 0x7F) as u16)]);
    }
    p
}

/// Select the array at byte `a`, load `d`, enable and run the unlock sequence, set WR.
fn write(map: Map, a: u16, d: u16, unlock: bool, wren: bool) -> Vec<u16> {
    let (adl, adh, dat, c1, c2, ..) = map;
    let mut p = set(map, &[(adh, 0x70), (adl, a), (dat, d), (c1, if wren { 0x44 } else { 0x40 })]);
    if unlock {
        p.extend(set(map, &[(c2, 0x55), (c2, 0xAA)]));
    }
    p.push(bsf((c1 & 0x7F) as u16, 1));
    p
}

fn step_to_end(m: &mut Machine16, n: usize) {
    for _ in 0..n {
        m.cpu.step();
    }
}

fn nvmif(m: &Machine16, map: Map) -> bool {
    m.cpu.mem[map.5] & map.6 != 0
}

#[test]
fn f171_and_f132_write_stalls_the_cpu_erases_first_and_sets_nvmif() {
    // PIC16F132xx: DS40002686B 15.2.2.2 p.174 (the processor stalls until the write is complete), MEM24 p.602 (11 ms maximum),
    // the same NVM registers and the EEPROM at 7000h (Table 15-1 p.159)
    for part in ["16F17146", "16F13245"] {
        let mut prog = write(P171, 5, 0x3C, true, true);
        let n = prog.len();
        prog.extend([NOP]);
        let mut m = machine(part, &prog);
        step_to_end(&mut m, n - 1);
        let c = m.cpu.cycles;
        assert!(!nvmif(&m, P171));
        m.cpu.step(); // BSF WR: the processor stalls until the byte is written
        assert_eq!(m.cpu.cycles - c, 1 + 11_000, "{part}");
        assert_eq!(m.cpu.mem[P171.3] & 0x02, 0, "WR cleared by hardware");
        assert!(nvmif(&m, P171));
        assert_eq!(m.cpu.mem[P171.3] & 0x44, 0x44, "NVMREGS and WREN stay");
        // read it back through FSR (7005h) with the extra cycle, and through RD
        let mut p = prog.clone();
        p.extend(set(P171, &[(P171.0, 5), (P171.3, 0x41)]));
        let mut m = machine(part, &p);
        step_to_end(&mut m, p.len());
        assert_eq!(m.cpu.mem[P171.2], 0x3C);
        assert_eq!(m.cpu.mem[P171.3] & 1, 0, "RD cleared at once");
    }
}

#[test]
fn f183_cpu_runs_on_during_the_write_and_wr_clears_after_4_ms() {
    let mut prog = write(P183, 9, 0xC3, true, true);
    let n = prog.len();
    prog.extend(std::iter::repeat_n(NOP, 4100));
    let mut m = machine("16F18346", &prog);
    step_to_end(&mut m, n);
    assert_eq!(m.cpu.mem[P183.3] & 0x02, 0x02, "WR set while the byte is written");
    assert!(!nvmif(&m, P183));
    let c = m.cpu.cycles;
    while m.cpu.cycles - c < 3900 {
        m.cpu.step();
    }
    assert_eq!(m.cpu.mem[P183.3] & 0x02, 0x02);
    while m.cpu.cycles - c < 4100 {
        m.cpu.step();
    }
    assert_eq!(m.cpu.mem[P183.3] & 0x02, 0);
    assert!(nvmif(&m, P183));
    assert_eq!(m.cpu.mem[P183.1], 0xF0, "NVMADRH bit 7 reads 1 (Register 11-4 p.137)");
}

#[test]
fn no_write_without_the_unlock_or_without_wren() {
    for (part, map) in [("16F17146", P171), ("16F13245", P171), ("16F18346", P183)] {
        for (unlock, wren) in [(false, true), (true, false)] {
            let mut prog = write(map, 5, 0x11, unlock, wren);
            prog.extend(std::iter::repeat_n(NOP, 20));
            let mut m = machine(part, &prog);
            step_to_end(&mut m, prog.len());
            assert_eq!(m.cpu.mem[map.3] & 0x02, 0, "{part} unlock {unlock} wren {wren}: WR stays clear");
            assert!(!nvmif(&m, map));
        }
    }
}

#[test]
fn a_reset_during_a_write_loses_it_and_sets_wrerr() {
    let mut prog = write(P183, 7, 0x00, true, true);
    prog.extend([NOP, RESET, NOP]);
    let mut m = machine("16F18346", &prog);
    step_to_end(&mut m, prog.len() - 2);
    assert_eq!(m.cpu.mem[P183.3] & 0x0A, 0x02, "WR set, WRERR clear");
    m.cpu.step(); // RESET
    assert_eq!(m.cpu.mem[P183.3] & 0x02, 0);
    assert_eq!(m.cpu.mem[P183.3] & 0x08, 0x08, "WRERR");
    assert_eq!(m.cpu.eeprom()[7], 0x5A, "the byte kept its old value (the image's)");
}

#[test]
fn power_on_reset_restores_the_image() {
    let mut prog = write(P171, 7, 0x00, true, true);
    prog.extend(set(P171, &[(P171.0, 7), (P171.3, 0x41)]));
    let mut m = machine("16F17146", &prog);
    step_to_end(&mut m, prog.len());
    assert_eq!(m.cpu.eeprom()[7], 0x00);
    m.reset().unwrap();
    assert_eq!(m.cpu.eeprom()[7], 0x5A, "power-on restores the image");
    let mut m = machine("16F17146", &set(P171, &[(P171.1, 0x70), (P171.0, 7), (P171.3, 0x41)]));
    step_to_end(&mut m, 12);
    assert_eq!(m.cpu.mem[P171.2], 0x5A);
}

/// Program flash self-write (DS40002343F 15.1.2 p.156-162): row erase, write latches, row write, word read.
#[test]
fn f171_flash_erase_latches_row_write_and_read() {
    let (adl, adh, datl, c1, c2) = (P171.0, P171.1, P171.2, P171.3, P171.4);
    let dath = datl + 1;
    let wr = bsf((c1 & 0x7F) as u16, 1);
    let unlock = || set(P171, &[(c2, 0x55), (c2, 0xAA)]);
    let mut segs: Vec<Vec<u16>> = Vec::new();
    // 0: erase the row of word 0105h (FREE, WREN)
    let mut p = set(P171, &[(adh, 0x01), (adl, 0x05), (c1, 0x14)]);
    p.extend(unlock());
    p.push(wr);
    segs.push(p);
    // 1: latch 5 = 1234h (LWLO, WREN): no operation
    let mut p = set(P171, &[(adl, 0x05), (datl, 0x34), (dath, 0x12), (c1, 0x24)]);
    p.extend(unlock());
    p.push(wr);
    segs.push(p);
    // 2: latch 6 = 0178h with LWLO = 0: the row is written
    let mut p = set(P171, &[(adl, 0x06), (datl, 0x78), (dath, 0x01), (c1, 0x04)]);
    p.extend(unlock());
    p.push(wr);
    segs.push(p);
    // 3: read word 0105h back
    segs.push(set(P171, &[(datl, 0), (dath, 0), (adl, 0x05), (c1, 0x01)]));
    // 4: write 3F00h over word 0106h (0178h): bits that need 0 -> 1 stay 0 (AND), with a note
    let mut p = set(P171, &[(adl, 0x06), (datl, 0x00), (dath, 0x3F), (c1, 0x04)]);
    p.extend(unlock());
    p.push(wr);
    segs.push(p);
    let all: Vec<u16> = segs.iter().flatten().copied().chain([NOP]).collect();
    let mut m = machine("16F17146", &all);
    let w = |m: &Machine16, i: usize| u16::from(m.cpu.prog[2 * i]) | u16::from(m.cpu.prog[2 * i + 1]) << 8;
    let run = |m: &mut Machine16, n: usize| -> u64 {
        let c = m.cpu.cycles;
        step_to_end(m, n);
        m.cpu.cycles - c
    };
    // the image words 0100h.. hold the program only if it is short; the test code is far below word 0100h
    assert!(all.len() < 0x100);
    let n0 = segs[0].len();
    run(&mut m, n0 - 1);
    assert_eq!(run(&mut m, 1), 1 + 2_000, "erase stalls the CPU 2 ms");
    assert!(nvmif(&m, P171));
    assert_eq!(m.cpu.mem[c1] & 0x02, 0);
    assert!((0x100..0x120).all(|i| w(&m, i) == 0x3FFF));
    m.cpu.mem[P171.5] &= !P171.6;
    assert_eq!(run(&mut m, segs[1].len()), segs[1].len() as u64, "a latch load takes no extra time");
    assert!(!nvmif(&m, P171), "a latch load sets no NVMIF");
    assert_eq!(w(&m, 0x105), 0x3FFF, "nothing written yet");
    run(&mut m, segs[2].len() - 1);
    assert_eq!(run(&mut m, 1), 1 + 2_000);
    assert!(nvmif(&m, P171));
    assert_eq!((w(&m, 0x105), w(&m, 0x106), w(&m, 0x107)), (0x1234, 0x0178, 0x3FFF));
    run(&mut m, segs[3].len());
    assert_eq!((m.cpu.mem[datl], m.cpu.mem[dath]), (0x34, 0x12), "RD reads the word");
    // latches are blank again: writing latch 6 only leaves word 5 alone; the AND keeps cleared bits cleared
    run(&mut m, segs[4].len() - 1);
    run(&mut m, 1);
    assert_eq!((w(&m, 0x105), w(&m, 0x106)), (0x1234, 0x0100));
    assert!(m.cpu.notes.iter().any(|n| n.contains("0 to 1")));
}
