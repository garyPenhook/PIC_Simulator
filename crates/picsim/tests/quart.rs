//! UART of the PIC18-Q41 (DS40002214G ch.34, PIC18F16Q41): transmitter, receiver, buffers and flags.

use devdesc::Device;
use picsim::Machine;
use std::path::Path;

fn machine(stim: &str) -> Machine {
    let dev = Device::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../devices/18F16Q41.toml")).unwrap();
    let mut m = Machine::new(dev).unwrap();
    m.load_stimulus(stim).unwrap();
    m
}

fn addr(m: &Machine, n: &str) -> u32 {
    m.dev.find_sfr(n).unwrap_or_else(|| panic!("no {n}")).addr
}

/// (address, mask) of a PIRx flag.
fn flag(m: &Machine, bit: &str) -> (usize, u8) {
    for s in m.dev.sfr.iter().filter(|s| s.name.starts_with("PIR")) {
        if let Some(f) = s.fields.iter().chain(s.legacy.iter()).find(|f| f.name == bit) {
            return (s.addr as usize, 1 << f.pos);
        }
    }
    panic!("no {bit}")
}

fn is_set(m: &Machine, bit: &str) -> bool {
    let (a, k) = flag(m, bit);
    m.cpu.m.mem[a] & k != 0
}

fn run_to(m: &mut Machine, c: u64) {
    while m.cpu.cycles < c {
        m.cpu.step();
    }
}

/// 4 instruction cycles per bit (BRGS = 1, UxBRG = 3), 8-bit mode, TXEN and RXEN as asked, then ON.
fn setup(m: &mut Machine, con0: u8, con2: u8) {
    let w = |m: &mut Machine, n: &str, v: u8| {
        let a = addr(m, n);
        m.cpu.write(a, v);
    };
    w(m, "U1BRGL", 3);
    w(m, "U1BRGH", 0);
    w(m, "U1CON0", con0);
    w(m, "U1CON2", con2);
    w(m, "U1CON1", 0x80);
}

fn rd(m: &mut Machine, n: &str) -> u8 {
    let a = addr(m, n);
    m.cpu.read(a)
}

#[test]
fn transmitter_timing_buffer_and_flags() {
    let mut m = machine("");
    setup(&mut m, 0xB0, 0);
    assert_eq!(rd(&mut m, "U1FIFO") & 0x30, 0x20, "TXBE set, TXBF clear");
    assert_ne!(rd(&mut m, "U1ERRIR") & 0x80, 0, "TXMTIF at reset");
    assert!(is_set(&m, "U1TXIF"), "TXIF once TXEN is set");
    run_to(&mut m, 100);
    let txb = addr(&m, "U1TXB");
    m.cpu.write(txb, b'A');
    assert_eq!(m.cpu.p.tx, vec![(1, 100, b'A')], "goes straight to the shift register");
    assert_eq!(rd(&mut m, "U1FIFO") & 0x30, 0x20, "UxTXB is free again");
    assert_eq!(rd(&mut m, "U1ERRIR") & 0x80, 0, "TXMTIF low while shifting");
    m.cpu.write(txb, b'B');
    assert_eq!(rd(&mut m, "U1FIFO") & 0x30, 0x10, "second byte waits: TXBF");
    assert!(!is_set(&m, "U1TXIF"));
    m.cpu.write(txb, b'C');
    assert_ne!(rd(&mut m, "U1FIFO") & 0x80, 0, "TXWRE: write into a full buffer");
    // The first character: Start + 8 data + Stop = 10 bits = 40 cycles. B moves to the TSR when A's Stop bit begins.
    run_to(&mut m, 100 + 36 - 1);
    assert!(!is_set(&m, "U1TXIF"));
    run_to(&mut m, 100 + 36 + 1);
    assert!(is_set(&m, "U1TXIF"), "UxTXIF rises at the beginning of the previous Stop bit");
    assert_eq!(m.cpu.p.tx.len(), 1);
    run_to(&mut m, 100 + 40 + 1);
    assert_eq!(m.cpu.p.tx, vec![(1, 100, b'A'), (1, 140, b'B')], "Start bit follows the last Stop bit");
    assert_eq!(rd(&mut m, "U1ERRIR") & 0x80, 0);
    run_to(&mut m, 100 + 80 + 1);
    assert_ne!(rd(&mut m, "U1ERRIR") & 0x80, 0, "TXMTIF at the end of the last Stop bit");
}

#[test]
fn data_written_before_txen_waits_and_stop_bits_count() {
    let mut m = machine("");
    setup(&mut m, 0x90, 0x10); // BRGS, RXEN; TXEN clear; STP = 01 (1.5 Stop bits)
    run_to(&mut m, 50);
    let txb = addr(&m, "U1TXB");
    m.cpu.write(txb, b'x');
    assert!(m.cpu.p.tx.is_empty(), "held in the TSR");
    assert_eq!(rd(&mut m, "U1ERRIR") & 0x80, 0, "TSR not empty");
    run_to(&mut m, 60);
    let c0 = addr(&m, "U1CON0");
    m.cpu.write(c0, 0xB0);
    assert_eq!(m.cpu.p.tx, vec![(1, 60, b'x')]);
    run_to(&mut m, 60 + 41);
    assert_eq!(rd(&mut m, "U1ERRIR") & 0x80, 0, "Start + 8 data + 1.5 Stop bits = 10.5 bits = 42 cycles");
    run_to(&mut m, 60 + 42);
    assert_ne!(rd(&mut m, "U1ERRIR") & 0x80, 0);
}

#[test]
fn flush_and_on_clear_and_brg_lock() {
    let mut m = machine("");
    setup(&mut m, 0xB0, 0);
    let (txb, fifo, con1) = (addr(&m, "U1TXB"), addr(&m, "U1FIFO"), addr(&m, "U1CON1"));
    m.cpu.write(txb, 1);
    m.cpu.write(txb, 2);
    assert_eq!(rd(&mut m, "U1FIFO") & 0x30, 0x10);
    m.cpu.write(fifo, 0x20 | 0x02); // TXBE = 1 flushes the buffer; the character in the TSR finishes
    assert_eq!(rd(&mut m, "U1FIFO") & 0x30, 0x20);
    run_to(&mut m, 200);
    assert_eq!(m.cpu.p.tx.len(), 1, "the flushed byte is never sent");
    // UxBRG takes a write only while ON = 0.
    let l = addr(&m, "U1BRGL");
    m.cpu.write(l, 9);
    assert_eq!(rd(&mut m, "U1BRGL"), 3);
    m.cpu.write(con1, 0);
    m.cpu.write(l, 9);
    assert_eq!(rd(&mut m, "U1BRGL"), 9);
    assert!(!is_set(&m, "U1TXIF"));
}

#[test]
fn byte_is_stored_in_the_middle_of_the_stop_bit() {
    let mut m = machine("[[uart]]\nat = 100\nbytes = \"A\"\n");
    setup(&mut m, 0xB0, 0);
    run_to(&mut m, 100 + 4 * 19 / 2 - 2);
    assert!(!is_set(&m, "U1RXIF"));
    assert_eq!(rd(&mut m, "U1FIFO") & 0x08, 0, "RXIDL low while the character comes in");
    run_to(&mut m, 100 + 4 * 19 / 2 + 2);
    assert!(is_set(&m, "U1RXIF"));
    assert_eq!(rd(&mut m, "U1FIFO") & 0x0B, 0x08, "RXIDL, RXBE clear, RXBF clear");
    assert_eq!(rd(&mut m, "U1RXB"), b'A');
    assert!(!is_set(&m, "U1RXIF"), "reading UxRXB empties the FIFO");
    assert_ne!(rd(&mut m, "U1FIFO") & 2, 0, "RXBE");
}

#[test]
fn third_character_overflows() {
    let mut m = machine("[[uart]]\nat = 10\nbytes = \"abc\"\n");
    setup(&mut m, 0xB0, 0);
    run_to(&mut m, 400);
    assert_eq!(rd(&mut m, "U1FIFO") & 3, 1, "RXBF");
    assert_ne!(rd(&mut m, "U1ERRIR") & 2, 0, "RXFOIF");
    assert_eq!(rd(&mut m, "U1RXB"), b'a');
    assert_eq!(rd(&mut m, "U1RXB"), b'b');
    assert_ne!(rd(&mut m, "U1FIFO") & 2, 0, "the third was discarded");
    assert_ne!(rd(&mut m, "U1ERRIR") & 2, 0, "RXFOIF stays until software clears it");
    let e = addr(&m, "U1ERRIR");
    m.cpu.write(e, 0);
    assert_eq!(rd(&mut m, "U1ERRIR") & 2, 0);
}

#[test]
fn framing_error_follows_the_top_character_and_can_suppress_rxif() {
    let mut m = machine("[[uart]]\nat = 10\ndata = [0x31]\n[[uart]]\ndata = [0x32]\nbad_stop = true\n");
    setup(&mut m, 0xB0, 0);
    let ie = addr(&m, "U1ERRIE");
    m.cpu.write(ie, 0x08); // FERIE
    run_to(&mut m, 200);
    assert_eq!(rd(&mut m, "U1ERRIR") & 8, 0, "top character is good");
    assert!(is_set(&m, "U1RXIF"));
    assert_eq!(rd(&mut m, "U1RXB"), 0x31);
    assert_ne!(rd(&mut m, "U1ERRIR") & 8, 0, "FERIF now describes the next one");
    assert!(!is_set(&m, "U1RXIF"), "FERIF with FERIE suppresses RXIF");
    assert!(is_set(&m, "U1EIF"), "summary error interrupt");
    assert_eq!(rd(&mut m, "U1RXB"), 0x32);
    assert_eq!(rd(&mut m, "U1ERRIR") & 8, 0);
}

#[test]
fn parity_mode_checks_the_ninth_bit() {
    // 0x41 has two ones; the parity bit makes the count even (MODE 0011) or odd (MODE 0010).
    let mut m = machine("[[uart]]\nat = 10\nnine = true\ndata = [0x41, 0x141]\n");
    setup(&mut m, 0xB3, 0);
    run_to(&mut m, 300);
    assert_eq!(rd(&mut m, "U1ERRIR") & 0x40, 0, "0x41 with parity 0: even");
    assert_eq!(rd(&mut m, "U1RXB"), 0x41);
    assert_ne!(rd(&mut m, "U1ERRIR") & 0x40, 0, "0x41 with parity 1: odd, wrong for even parity");
    assert_eq!(rd(&mut m, "U1RXB"), 0x41);
    assert_eq!(rd(&mut m, "U1ERRIR") & 0x40, 0);
}

#[test]
fn rx_flush_and_on_reset() {
    let mut m = machine("[[uart]]\nat = 10\nbytes = \"ab\"\n");
    setup(&mut m, 0xB0, 0);
    run_to(&mut m, 300);
    assert_eq!(rd(&mut m, "U1FIFO") & 3, 1);
    let (fifo, con1) = (addr(&m, "U1FIFO"), addr(&m, "U1CON1"));
    m.cpu.write(fifo, 0x02);
    assert_ne!(rd(&mut m, "U1FIFO") & 2, 0, "RXBE = 1 flushed the FIFO");
    assert!(!is_set(&m, "U1RXIF"));
    m.cpu.write(con1, 0);
    assert!(!is_set(&m, "U1TXIF"));
    assert_ne!(rd(&mut m, "U1ERRIR") & 0x80, 0);
}
