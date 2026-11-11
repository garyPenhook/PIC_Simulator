//! Program flash self-write: PIC18F27Q10 (DS40002043F 12.1) and PIC18F16Q41 (DS40002214G 10.3).

use devdesc::Device;
use picsim::Machine;
use std::path::Path;

fn machine(part: &str) -> Machine {
    let dev = Device::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("../../devices/{part}.toml"))).unwrap();
    let mut m = Machine::new(dev).unwrap();
    m.cpu.p.board.fosc_mhz = 4.0; // the cycle counts below are at Fosc = 4 MHz (1 000 instruction cycles per ms)
    m
}

fn a(m: &Machine, n: &str) -> u32 {
    m.dev.find_sfr(n).unwrap_or_else(|| panic!("no {n}")).addr
}

fn setadr(m: &mut Machine, adr: u32) {
    for (n, sh) in [("NVMADRL", 0), ("NVMADRH", 8), ("NVMADRU", 16)] {
        let r = a(m, n);
        m.cpu.write(r, (adr >> sh) as u8);
    }
}

fn setdat(m: &mut Machine, v: u16) {
    let (l, h) = (a(m, "NVMDATL"), a(m, "NVMDATH"));
    m.cpu.write(l, v as u8);
    m.cpu.write(h, (v >> 8) as u8);
}

fn dat(m: &mut Machine) -> u16 {
    let (l, h) = (a(m, "NVMDATL"), a(m, "NVMDATH"));
    u16::from(m.cpu.read(l)) | u16::from(m.cpu.read(h)) << 8
}

/// Run `TBLWT*+` n times from TABLAT values `data`, with TBLPTR = `ptr`: the instruction itself, at the reset vector.
fn tblwt(m: &mut Machine, ptr: u32, data: &[u8]) {
    for (n, sh) in [("TBLPTRL", 0), ("TBLPTRH", 8), ("TBLPTRU", 16)] {
        let r = a(m, n);
        m.cpu.write(r, (ptr >> sh) as u8);
    }
    let ta = a(m, "TABLAT");
    for (i, d) in data.iter().enumerate() {
        m.cpu.prog[2 * i] = 0x0D; // TBLWT*+
        m.cpu.prog[2 * i + 1] = 0x00;
        m.cpu.write(ta, *d);
        m.cpu.pc = 2 * i as u32;
        m.cpu.step();
    }
    m.cpu.pc = 0;
}

/// Writes to NVMCON2 (twice) and NVMCON1; returns the cycles the last write stalled for.
fn q10_op(m: &mut Machine, unlock: (u8, u8), bit: u8) -> u64 {
    let (c1, c2) = (a(m, "NVMCON1"), a(m, "NVMCON2"));
    m.cpu.write(c2, unlock.0);
    m.cpu.write(c2, unlock.1);
    let t = m.cpu.cycles;
    m.cpu.write(c1, bit);
    m.cpu.cycles - t
}

fn q10_en(m: &mut Machine) {
    let c0 = a(m, "NVMCON0");
    m.cpu.write(c0, 0x80);
}

fn pir7_nvmif(m: &mut Machine) -> bool {
    let r = a(m, "PIR7");
    m.cpu.read(r) & 0x20 != 0
}

#[test]
fn q10_erase_write_and_read_back() {
    let mut m = machine("18F27Q10");
    let (c0, c1, c7) = (a(&m, "NVMCON0"), a(&m, "NVMCON1"), a(&m, "PIR7"));
    m.cpu.prog[0x1F00..0x2000].fill(0x00);
    q10_en(&mut m);
    setadr(&mut m, 0x1F10); // inside the sector at 0x1F00
    let stall = q10_op(&mut m, (0xCC, 0x33), 0x40);
    assert_eq!(stall, 10_000, "10 ms typ at 1 MHz");
    assert!(m.cpu.prog[0x1F00..0x2000].iter().all(|b| *b == 0xFF));
    assert_eq!(m.cpu.read(c1) & 0x40, 0, "SECER clear after the stall");
    assert!(pir7_nvmif(&mut m));
    m.cpu.write(c7, 0);
    // word write 1234h at 0x1F10
    setdat(&mut m, 0x1234);
    assert_eq!(q10_op(&mut m, (0x55, 0xAA), 0x10), 50);
    assert_eq!((m.cpu.prog[0x1F10], m.cpu.prog[0x1F11]), (0x34, 0x12));
    assert!(pir7_nvmif(&mut m));
    // read it back, with NVMEN clear
    m.cpu.write(c0, 0);
    setdat(&mut m, 0);
    m.cpu.write(c1, 1);
    assert_eq!(dat(&mut m), 0x1234);
}

#[test]
fn q10_sector_read_tblwt_sector_write() {
    let mut m = machine("18F27Q10");
    let hold = m.dev.memory.data.gpr.iter().find(|g| g.system).unwrap().start as usize;
    assert_eq!(hold, 0xD00);
    m.cpu.prog[0x1E00..0x1F00].copy_from_slice(&(0..=255u8).collect::<Vec<_>>());
    q10_en(&mut m);
    setadr(&mut m, 0x1E40);
    assert_eq!(q10_op(&mut m, (0xBB, 0x44), 0x02), 0, "SECRD");
    assert_eq!(m.cpu.m.mem[hold + 7], 7);
    assert_eq!(m.cpu.m.mem[hold + 255], 255);
    assert_eq!(q10_op(&mut m, (0xCC, 0x33), 0x40), 10_000);
    tblwt(&mut m, 0x20, &[0xAB, 0xCD]); // TBLPTR[7:0] picks the holding register
    assert_eq!((m.cpu.m.mem[hold + 0x20], m.cpu.m.mem[hold + 0x21]), (0xAB, 0xCD));
    let t = m.cpu.cycles;
    assert_eq!(q10_op(&mut m, (0xDD, 0x22), 0x20), 6_000);
    assert!(m.cpu.cycles > t);
    assert_eq!((m.cpu.prog[0x1E20], m.cpu.prog[0x1E21]), (0xAB, 0xCD));
    assert_eq!(m.cpu.prog[0x1E07], 7, "the rest of the sector keeps what SECRD read");
}

#[test]
fn q10_gating_and_errors() {
    let mut m = machine("18F27Q10");
    let c0 = a(&m, "NVMCON0");
    m.cpu.prog[0x1F00..0x2000].fill(0x00);
    setadr(&mut m, 0x1F00);
    assert_eq!(q10_op(&mut m, (0xCC, 0x33), 0x40), 0);
    assert_eq!(m.cpu.prog[0x1F00], 0, "NVMEN clear: no erase");
    q10_en(&mut m);
    assert_eq!(q10_op(&mut m, (0x55, 0xAA), 0x40), 0);
    assert_eq!(m.cpu.prog[0x1F00], 0, "the word-write unlock does not enable SECER");
    assert_eq!(q10_op(&mut m, (0xCC, 0x33), 0x40), 10_000);
    setadr(&mut m, 0x2_0000); // one past the 128 KB flash
    assert_eq!(q10_op(&mut m, (0xCC, 0x33), 0x40), 0);
    assert_eq!(m.cpu.read(c0) & 0x10, 0x10, "NVMERR");
}

fn q41_go(m: &mut Machine, cmd: u8, unlock: bool) -> u64 {
    let (c0, c1, lock) = (a(m, "NVMCON0"), a(m, "NVMCON1"), a(m, "NVMLOCK"));
    m.cpu.write(c1, cmd);
    if unlock {
        m.cpu.write(lock, 0x55);
        m.cpu.write(lock, 0xAA);
    }
    let t = m.cpu.cycles;
    m.cpu.write(c0, 1);
    m.cpu.cycles - t
}

#[test]
fn q41_page_erase_write_read_and_word_ops() {
    let mut m = machine("18F16Q41");
    let (c0, pir0) = (a(&m, "NVMCON0"), a(&m, "PIR0"));
    let hold = m.dev.memory.data.gpr.iter().find(|g| g.system).unwrap().start as usize;
    assert_eq!(hold, 0x1500, "Table 10-2: bank 21 on the Fx6Q41");
    m.cpu.prog[0x1F00..0x2000].copy_from_slice(&(0..=255u8).collect::<Vec<_>>());
    setadr(&mut m, 0x1F02);
    assert_eq!(q41_go(&mut m, 2, false), 0, "page read");
    assert_eq!(m.cpu.m.mem[hold + 9], 9);
    assert_eq!(q41_go(&mut m, 6, true), 11_000, "page erase");
    assert!(m.cpu.prog[0x1F00..0x2000].iter().all(|b| *b == 0xFF));
    assert_eq!(m.cpu.read(c0) & 1, 0);
    assert_eq!(m.cpu.read(pir0) & 0x10, 0x10, "NVMIF");
    tblwt(&mut m, 0x40, &[0x11, 0x22, 0x33]);
    assert_eq!(m.cpu.m.mem[hold + 0x42], 0x33);
    assert_eq!(q41_go(&mut m, 5, true), 10_000, "page write");
    assert_eq!(&m.cpu.prog[0x1F40..0x1F43], &[0x11, 0x22, 0x33]);
    assert_eq!(m.cpu.prog[0x1F09], 9, "a buffer byte TBLWT did not touch still holds what the page read put there");
}

#[test]
fn q41_word_write_read_post_increment_and_errors() {
    let mut m = machine("18F16Q41");
    let c1 = a(&m, "NVMCON1");
    setadr(&mut m, 0x1000);
    setdat(&mut m, 0xBEEF);
    assert_eq!(q41_go(&mut m, 4, true), 75, "word write with post increment");
    assert_eq!((m.cpu.prog[0x1000], m.cpu.prog[0x1001]), (0xEF, 0xBE));
    let r = [a(&m, "NVMADRL"), a(&m, "NVMADRH")];
    assert_eq!((m.cpu.read(r[0]), m.cpu.read(r[1])), (0x02, 0x10), "NVMADR += 2 on the PFM");
    setadr(&mut m, 0x1000);
    setdat(&mut m, 0);
    q41_go(&mut m, 0, false);
    assert_eq!(dat(&mut m), 0xBEEF);
    // no unlock, and an address past the 64 KB flash
    setdat(&mut m, 0x1111);
    q41_go(&mut m, 3, false);
    assert_eq!(m.cpu.read(c1) & 0x80, 0x80);
    m.cpu.write(c1, 0);
    setadr(&mut m, 0x1_0000);
    q41_go(&mut m, 3, true);
    assert_eq!(m.cpu.read(c1) & 0x80, 0x80, "WRERR for an invalid address");
}
