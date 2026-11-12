//! What a reset other than power-on keeps on a PIC18, and EEPROM data from the hex file (REVIEW-2026-11-11 B9-B11).

use devdesc::Device;
use picsim::Machine;
use picsim::periph::ResetKind;
use std::path::Path;

fn machine(part: &str) -> Machine {
    let dev = Device::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("../../devices/{part}.toml"))).unwrap();
    Machine::new(dev).unwrap()
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

/// Q10 EEPROM read: NVMCON0 = 0 (data EEPROM), NVMCON1.RD.
fn ee_read_q10(m: &mut Machine, adr: u32) -> u8 {
    setadr(m, adr);
    let (c0, c1, d) = (a(m, "NVMCON0"), a(m, "NVMCON1"), a(m, "NVMDATL"));
    m.cpu.write(c0, 0);
    m.cpu.write(c1, 1);
    m.cpu.read(d)
}

#[test]
fn eeprom_survives_a_warm_reset() {
    let mut m = machine("18F27Q10");
    let (d, c0, c1, c2) = (a(&m, "NVMDATL"), a(&m, "NVMCON0"), a(&m, "NVMCON1"), a(&m, "NVMCON2"));
    setadr(&mut m, 0x310005);
    m.cpu.write(d, 0xA7);
    m.cpu.write(c0, 0x80);
    m.cpu.write(c2, 0x55);
    m.cpu.write(c2, 0xAA);
    m.cpu.write(c1, 0x10);
    let t = m.cpu.cycles + 200_000; // 10 ms at the default 64 MHz is 160 000 cycles
    while m.cpu.cycles < t {
        m.cpu.step();
    }
    assert_eq!(ee_read_q10(&mut m, 0x310005), 0xA7);
    m.cpu.reset_cond(ResetKind::Instruction);
    assert_eq!(ee_read_q10(&mut m, 0x310005), 0xA7, "after RESET");
    m.cpu.reset_cond(ResetKind::Wdt);
    assert_eq!(ee_read_q10(&mut m, 0x310005), 0xA7, "after a watchdog reset");
}

#[test]
fn eeprom_data_in_the_hex_file_is_loaded() {
    let mut m = machine("18F27Q10");
    // 5Ah at 310003h
    m.load_hex(":020000040031C9\n:010003005AA2\n:00000001FF\n").unwrap();
    assert_eq!(ee_read_q10(&mut m, 0x310003), 0x5A);
    assert_eq!(ee_read_q10(&mut m, 0x310002), 0xFF);
    m.cpu.reset_cond(ResetKind::Mclr);
    assert_eq!(ee_read_q10(&mut m, 0x310003), 0x5A, "after MCLR");
}

#[test]
fn eeprom_data_in_the_hex_file_is_loaded_on_a_q41() {
    let mut m = machine("18F16Q41");
    let start = m.dev.memory.eeprom.start;
    let ext = start >> 16;
    let sum = |b: &[u8]| (0u8).wrapping_sub(b.iter().fold(0u8, |s, x| s.wrapping_add(*x)));
    let r4 = [2, 0, 0, 4, (ext >> 8) as u8, ext as u8];
    let lo = start & 0xFFFF;
    let r0 = [1, (lo >> 8) as u8, lo as u8, 0, 0x3C];
    let hex = |b: &[u8]| format!(":{}{:02X}\n", b.iter().map(|x| format!("{x:02X}")).collect::<String>(), sum(b));
    m.load_hex(&format!("{}{}:00000001FF\n", hex(&r4), hex(&r0))).unwrap();
    assert_eq!(m.cpu.p.eeprom().unwrap()[0], 0x3C);
    m.cpu.reset_cond(ResetKind::Instruction);
    assert_eq!(m.cpu.p.eeprom().unwrap()[0], 0x3C);
}

#[test]
fn the_board_survives_a_warm_reset() {
    let mut m = machine("18F27Q10");
    m.load_stimulus("[[led]]\npin = \"RB0\"\n").unwrap();
    let (tris, lat) = (a(&m, "TRISB"), a(&m, "LATB"));
    m.cpu.write(tris, 0xFE);
    for v in [1, 0, 1, 0, 1] {
        m.cpu.write(lat, v);
        m.cpu.cycles += 10;
        let now = m.cpu.cycles;
        m.cpu.p.board_poll(&m.cpu.m, now);
    }
    let (n, on) = (m.cpu.p.board.leds[0].changes.len(), m.cpu.p.board.leds[0].on_cycles);
    assert!(n >= 5 && on > 0, "{n} {on}");
    m.cpu.reset_cond(ResetKind::Wdt);
    assert_eq!(m.cpu.p.board.leds.len(), 1);
    assert!(m.cpu.p.board.leds[0].changes.len() >= n, "history kept");
    assert!(m.cpu.p.board.leds[0].on_cycles >= on, "on-time kept");
}

#[test]
fn a_reset_from_the_cached_peripherals_matches_a_fresh_bind() {
    for part in ["18F27Q10", "18F16Q41"] {
        let mut m = machine(part);
        m.load_stimulus("[[led]]\npin = \"RB4\"\n").unwrap(); // RB4: the 20-pin Q41 has no RB0-RB3
        m.cpu.reset_cond(ResetKind::Instruction); // binds and caches
        let (mem, special) = (m.cpu.m.mem.clone(), m.cpu.m.special.clone());
        m.cpu.reset_cond(ResetKind::Instruction); // from the cache
        assert_eq!(m.cpu.m.mem, mem, "{part}");
        assert_eq!(m.cpu.m.special, special, "{part}");
        assert_eq!(m.cpu.p.board.leds.len(), 1, "{part}: stimulus board kept");
    }
}

/// One SPI byte through MSSP1 at Fosc/4: what the client answered.
fn spi_byte(m: &mut Machine) -> u8 {
    let (con, buf) = (a(m, "SSP1CON1"), a(m, "SSP1BUF"));
    m.cpu.write(con, 0x20); // SSPEN, SSPM = 0000: Fosc/4
    m.cpu.write(buf, 0x00);
    let end = m.cpu.cycles + 8;
    while m.cpu.cycles < end {
        m.cpu.step();
    }
    m.cpu.read(buf)
}

#[test]
fn an_spi_client_goes_on_after_a_warm_reset() {
    let mut m = machine("18F27Q10");
    m.load_stimulus("[[spi]]\ndata = [0xA5, 0x3C]\n").unwrap();
    assert_eq!(spi_byte(&mut m), 0xA5);
    m.cpu.reset_cond(ResetKind::Wdt);
    assert_eq!(spi_byte(&mut m), 0x3C, "the outside device was not reset");
}
