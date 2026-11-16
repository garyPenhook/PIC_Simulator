//! Q41 DMA behavior from DS40002214G ch.16. These exercise the engine, not just stored registers.
use devdesc::Device;
use picsim::{Machine, periph::ResetKind};
use std::path::Path;
const PARTS: [&str; 6] = ["18F04Q41", "18F05Q41", "18F06Q41", "18F14Q41", "18F15Q41", "18F16Q41"];
fn machine(part: &str) -> Machine {
    let d = Device::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("../../devices/{part}.toml"))).unwrap();
    let mut m = Machine::new(d).unwrap();
    m.cpu.prog[..256].fill(0); // NOPs
    m
}
fn w(m: &mut Machine, n: &str, v: u8) {
    m.cpu.write(m.dev.find_sfr(n).unwrap().addr, v);
}
fn r(m: &mut Machine, n: &str) -> u8 {
    m.cpu.read(m.dev.find_sfr(n).unwrap().addr)
}
fn pair(m: &mut Machine, name: &str, value: u16) {
    w(m, &format!("DMAn{name}L"), value as u8);
    w(m, &format!("DMAn{name}H"), (value >> 8) as u8);
}
fn setup(m: &mut Machine, channel: u8, source: u16, dest: u16, ss: u16, ds: u16, mode: u8) {
    w(m, "DMASELECT", channel);
    pair(m, "SSA", source);
    pair(m, "DSA", dest);
    pair(m, "SSZ", ss);
    pair(m, "DSZ", ds);
    w(m, "DMAnCON1", mode);
}
fn steps(m: &mut Machine, n: usize) {
    for _ in 0..n {
        m.cpu.step();
    }
}
#[test]
fn all_parts_copy_reload_and_complete_flags() {
    for part in PARTS {
        let mut m = machine(part);
        for i in 0..4 {
            m.cpu.write(0x100 + i, 0x31 + i as u8);
        }
        setup(&mut m, 0, 0x100, 0x120, 4, 4, 0x63); // increment both, stop both
        w(&mut m, "DMAnCON0", 0xE0);
        let pc = m.cpu.pc;
        steps(&mut m, 8);
        assert_eq!(m.cpu.pc, pc, "DMA bus cycles stall CPU");
        for i in 0..4 {
            assert_eq!(m.cpu.read(0x120 + i), 0x31 + i as u8, "{part}");
        }
        assert_eq!(r(&mut m, "DMAnCON0"), 0x80);
        assert_eq!(r(&mut m, "DMAnSCNTL"), 4);
        assert_eq!(r(&mut m, "DMAnDCNTL"), 4);
        assert_eq!(r(&mut m, "DMAnSPTRL"), 0);
        assert_eq!(r(&mut m, "DMAnDPTRL"), 0x20);
        assert_eq!(r(&mut m, "PIR2") & 0x30, 0x30);
    }
}
#[test]
fn banks_peek_read_only_and_invalid_selection() {
    let mut m = machine(PARTS[5]);
    for c in 0..4 {
        setup(&mut m, c, 0x100 + c as u16, 0x120, 1, 1, 0);
    }
    for c in 0..4 {
        w(&mut m, "DMASELECT", c);
        assert_eq!(r(&mut m, "DMAnSSAL"), c);
        w(&mut m, "DMAnCON0", 0x80);
        w(&mut m, "DMAnSPTRL", 0xEE);
        let a = m.dev.find_sfr("DMAnSPTRL").unwrap().addr;
        assert_eq!(m.cpu.peek(a), c);
    }
    w(&mut m, "DMASELECT", 7);
    w(&mut m, "DMAnCON0", 0xFF);
    assert_eq!(r(&mut m, "DMAnCON0"), 0);
}
#[test]
fn soft_stop_retains_buffer_hard_stop_reinitializes() {
    let mut m = machine(PARTS[5]);
    m.cpu.write(0x100, 0xA5);
    setup(&mut m, 0, 0x100, 0x120, 2, 2, 0x42);
    w(&mut m, "DMAnCON0", 0xA0);
    steps(&mut m, 1);
    assert_eq!(r(&mut m, "DMAnCON0") & 1, 1);
    w(&mut m, "DMAnCON0", 0x80);
    steps(&mut m, 2);
    assert_eq!(m.cpu.read(0x120), 0);
    m.cpu.write(0x100, 0x66);
    w(&mut m, "DMAnCON0", 0xA0);
    steps(&mut m, 1);
    assert_eq!(m.cpu.read(0x120), 0xA5);
    w(&mut m, "DMAnCON0", 0);
    w(&mut m, "DMAnCON0", 0xA0);
    steps(&mut m, 2);
    assert_eq!(m.cpu.read(0x120), 0x66);
}
#[test]
fn unequal_sizes_fixed_and_decrement_modes() {
    let mut m = machine(PARTS[5]);
    m.cpu.write(0x101, 0xAB);
    m.cpu.write(0x100, 0xCD);
    setup(&mut m, 0, 0x101, 0x123, 2, 4, 0x84);
    w(&mut m, "DMAnCON0", 0xA0);
    steps(&mut m, 4);
    assert_eq!(m.cpu.read(0x123), 0xAB);
    assert_eq!(m.cpu.read(0x122), 0xCD);
    assert_eq!(r(&mut m, "DMAnSCNTL"), 2);
    assert_eq!(r(&mut m, "DMAnDCNTL"), 2);
    w(&mut m, "DMAnCON0", 0xA0);
    steps(&mut m, 4);
    assert_eq!(m.cpu.read(0x121), 0xAB);
    assert_eq!(m.cpu.read(0x120), 0xCD);
}
#[test]
fn flash_and_eeprom_sources_and_sfr_side_effects() {
    let mut m = machine(PARTS[5]);
    m.cpu.prog[0x300..0x303].copy_from_slice(&[0x12, 0x34, 0x56]);
    setup(&mut m, 0, 0x300, 0x120, 3, 3, 0x4A);
    w(&mut m, "DMAnCON0", 0xA0);
    steps(&mut m, 6);
    assert_eq!((m.cpu.read(0x120), m.cpu.read(0x122)), (0x12, 0x56));
    m.cpu.p.load_eeprom(|a| if a & 0xFFFF == 0 { Some(0x9A) } else { None });
    setup(&mut m, 0, 0, 0x120, 1, 1, 0x10);
    w(&mut m, "DMAnCON0", 0);
    w(&mut m, "DMAnCON0", 0xA0);
    steps(&mut m, 2);
    assert_eq!(m.cpu.read(0x120), 0x9A);
    // Timer register destination must use its model, not write the backing memory alone.
    m.cpu.write(0x100, 0x73);
    let t = m.dev.find_sfr("T4TMR").unwrap().addr as u16;
    setup(&mut m, 0, 0x100, t, 1, 1, 0);
    w(&mut m, "DMAnCON0", 0);
    w(&mut m, "DMAnCON0", 0xA0);
    steps(&mut m, 2);
    assert_eq!(r(&mut m, "T4TMR"), 0x73);
}
#[test]
fn timer_hardware_start_abort_and_pmd() {
    let mut m = machine(PARTS[5]);
    m.cpu.write(0x100, 0x5A);
    setup(&mut m, 0, 0x100, 0x120, 32, 32, 0x42);
    w(&mut m, "DMAnSIRQ", 0x1B); // Timer2, independent of PIE/GIE
    w(&mut m, "DMAnAIRQ", 0x53); // Timer4
    w(&mut m, "DMAnCON0", 0xC4);
    w(&mut m, "T2CLKCON", 1);
    w(&mut m, "T2PR", 3);
    w(&mut m, "T2CON", 0x80);
    steps(&mut m, 8);
    assert_eq!(m.cpu.read(0x120), 0x5A);
    w(&mut m, "T4CLKCON", 1);
    w(&mut m, "T4PR", 1);
    w(&mut m, "T4CON", 0x80);
    steps(&mut m, 4);
    assert_eq!(r(&mut m, "DMAnCON0") & 0x64, 0);
    assert_ne!(r(&mut m, "PIR2") & 0x80, 0);
    w(&mut m, "T2CON", 0);
    w(&mut m, "T4CON", 0);
    let pmd = m
        .dev
        .sfr
        .iter()
        .find_map(|s| s.fields.iter().find(|f| f.name == "DMA1MD").map(|f| (s.addr, 1 << f.pos)))
        .unwrap();
    m.cpu.write(pmd.0, pmd.1);
    w(&mut m, "DMAnCON0", 0xA0);
    let before = r(&mut m, "DMAnDCNTL");
    steps(&mut m, 4);
    assert_eq!(r(&mut m, "DMAnDCNTL"), before);
    m.cpu.reset_cond(ResetKind::Instruction);
    w(&mut m, "DMASELECT", 0);
    assert_eq!(r(&mut m, "DMAnCON0"), 0);
}
#[test]
fn uart_level_request_handshake_and_source_stop() {
    let mut m = machine(PARTS[5]);
    for (i, &b) in b"DMA".iter().enumerate() {
        m.cpu.write(0x100 + i as u32, b);
    }
    let tx = m.dev.find_sfr("U1TXB").unwrap().addr as u16;
    setup(&mut m, 0, 0x100, tx, 3, 1, 3); // increment source; fixed destination; SSTP
    w(&mut m, "DMAnSIRQ", 0x21);
    w(&mut m, "DMAnCON0", 0xC0);
    w(&mut m, "U1BRGL", 1);
    w(&mut m, "U1CON0", 0xA0);
    w(&mut m, "U1CON1", 0x80);
    steps(&mut m, 100);
    let bytes: Vec<_> = m.cpu.p.tx.iter().map(|x| x.2).collect();
    assert_eq!(bytes, b"DMA");
    assert_eq!(r(&mut m, "DMAnCON0") & 0x60, 0);
    assert_eq!(r(&mut m, "PIR2") & 0x40, 0, "a level request never causes overrun");
}
#[test]
fn repeated_timer_pulses_overrun_even_when_timer_flag_stays_set() {
    let mut m = machine(PARTS[5]);
    setup(&mut m, 0, 0x100, 0x120, 30, 30, 0x42);
    w(&mut m, "DMAnSIRQ", 0x1B);
    w(&mut m, "DMAnCON0", 0xC0);
    w(&mut m, "T2CLKCON", 1);
    w(&mut m, "T2PR", 1);
    w(&mut m, "T2CON", 0x80);
    steps(&mut m, 10);
    assert_ne!(r(&mut m, "PIR2") & 0x40, 0);
    assert_ne!(r(&mut m, "DMAnCON0") & 0x20, 0, "overrun does not abort");
}
#[test]
fn completion_dispatches_q41_vectored_interrupt() {
    let mut m = machine(PARTS[5]);
    m.cpu.pc = 0x200;
    m.cpu.prog[8 + 2 * 0x14] = 0x40; // DMA1SCNT vector -> ISR 0x100
    m.cpu.prog[9 + 2 * 0x14] = 0;
    setup(&mut m, 0, 0x100, 0x120, 1, 1, 0);
    w(&mut m, "PIE2", 0x10);
    w(&mut m, "INTCON0", 0x80);
    w(&mut m, "DMAnCON0", 0xA0);
    steps(&mut m, 3);
    assert_eq!(m.cpu.pc, 0x100);
    assert_eq!(m.cpu.in_service, vec![true]);
}
#[test]
fn independent_channels_can_chain_on_completion() {
    let mut m = machine(PARTS[5]);
    for i in 0..3 {
        m.cpu.write(0x100 + i, 0x81 + i as u8);
    }
    setup(&mut m, 1, 0x120, 0x140, 3, 3, 0x63);
    w(&mut m, "DMAnSIRQ", 0x15);
    w(&mut m, "DMAnCON0", 0xC0);
    setup(&mut m, 0, 0x100, 0x120, 3, 3, 0x63);
    w(&mut m, "DMAnCON0", 0xA0);
    steps(&mut m, 12);
    for i in 0..3 {
        assert_eq!(m.cpu.read(0x140 + i), 0x81 + i as u8);
    }
    w(&mut m, "DMASELECT", 1);
    assert_eq!(r(&mut m, "DMAnCON0"), 0x80);
    assert_eq!(r(&mut m, "PIR6") & 0x30, 0x30);
}
