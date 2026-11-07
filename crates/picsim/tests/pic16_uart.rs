//! The EUSART on the PIC16 core (PIC16F17146, DS40002343F ch.32 p.453-484): the same module as the Q10's, shared through
//! `periph/uart.rs`. TX1REG, TX1STA, RC1STA, BAUD1CON and SP1BRGL/H are in bank 14 (070Ch-0712h); TX1IF and RC1IF are
//! PIR4 bits 6 and 7 (0090h), PIE4 is 009Ah.

use devdesc::Device;
use picsim::machine16::Machine16;
use std::path::Path;

fn movlw(k: u16) -> u16 { 0x3000 | k }
fn movwf(f: u16) -> u16 { 0x0080 | f }
fn movf_w(f: u16) -> u16 { 0x0800 | f }
fn bsf(f: u16, b: u16) -> u16 { 0x1400 | b << 7 | f }
fn movlb(k: u16) -> u16 { 0x0140 | k }
const NOP: u16 = 0;
const GOTO_SELF: u16 = 0x2800; // GOTO k is GOTO_SELF + k

const RC1REG: u16 = 0x0C;
const TX1REG: u16 = 0x0D;
const RC1STA: u16 = 0x10;
const TX1STA: u16 = 0x11;
const PIR4: usize = 0x90;
const TXIF: u8 = 0x40;
const RCIF: u8 = 0x80;

fn hexrec(b: &[u8]) -> String {
    let sum = b.iter().fold(0u8, |a, &x| a.wrapping_add(x));
    format!(":{}{:02X}\n", b.iter().map(|x| format!("{x:02X}")).collect::<String>(), sum.wrapping_neg())
}

/// The part with `prog` at word 0 and `isr` at the interrupt vector (word 4), run from reset.
fn machine(prog: &[u16], isr: &[u16], stim: Option<&str>) -> Machine16 {
    let dev = Device::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../devices/16F17146.toml")).unwrap();
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
    if let Some(s) = stim {
        m.load_stimulus(s).unwrap();
    }
    m
}

fn run_to(m: &mut Machine16, c: u64) {
    while m.cpu.cycles < c {
        m.cpu.step();
    }
}

fn pir4(m: &Machine16) -> u8 {
    m.cpu.mem[PIR4]
}

/// TXEN, 8-bit BRG, SP1BRG = 0: 16 cycles a bit, 160 a character. TX1REG is written by the 5th instruction (cycle 5).
fn tx_prog(second: bool) -> Vec<u16> {
    let mut p = vec![movlb(14), movlw(0x20), movwf(TX1STA), movlw(b'A' as u16), movwf(TX1REG)];
    if second {
        p.extend([movlw(b'B' as u16), movwf(TX1REG)]);
    }
    p.extend([NOP; 400]);
    p
}

#[test]
fn transmitter_timing_flags_and_back_to_back() {
    let mut m = machine(&tx_prog(true), &[], None);
    run_to(&mut m, 3);
    assert_eq!(pir4(&m) & TXIF, TXIF, "TXEN set and nothing held: TXIF");
    run_to(&mut m, 5);
    assert_eq!(m.cpu.tx, vec![(1, 5, b'A')], "goes straight to the shift register");
    run_to(&mut m, 7);
    assert_eq!(pir4(&m) & TXIF, 0, "B is held in TX1REG");
    assert_eq!(m.cpu.mem[0x700 + TX1STA as usize] & 2, 0, "TRMT clear while shifting");
    run_to(&mut m, 164);
    assert_eq!(m.cpu.tx.len(), 1);
    run_to(&mut m, 166);
    assert_eq!(m.cpu.tx, vec![(1, 5, b'A'), (1, 165, b'B')], "the Start bit follows the Stop bit");
    assert_eq!(pir4(&m) & TXIF, TXIF, "TX1REG empty again");
    run_to(&mut m, 324);
    assert_eq!(m.cpu.mem[0x700 + TX1STA as usize] & 2, 0, "B is still shifting until 165 + 160 = 325");
    run_to(&mut m, 326);
    assert_eq!(m.cpu.mem[0x700 + TX1STA as usize] & 2, 2, "TRMT set when the character is out");
}

#[test]
fn receiver_flag_register_and_two_deep_fifo() {
    // SPEN | CREN; 16 cycles a bit; a character reaches the FIFO half a bit into its Stop bit: 20 + 16 * 19 / 2 = 172
    let prog: Vec<u16> = [vec![movlb(14), movlw(0x90), movwf(RC1STA)], vec![NOP; 400]].concat();
    let stim = "[[uart]]\nport = 1\nbytes = \"UV\"\nat = 20\n";
    let mut m = machine(&prog, &[], Some(stim));
    run_to(&mut m, 170);
    assert_eq!(pir4(&m) & RCIF, 0, "20 + 152: the Stop bit is sampled half a bit time in");
    run_to(&mut m, 175);
    assert_eq!(pir4(&m) & RCIF, RCIF, "first character in the FIFO");
    run_to(&mut m, 330);
    assert_eq!(m.cpu.mem[0x700 + RC1STA as usize] & 2, 0, "no overrun with two characters");
    // read both through the register
    let mut m = machine(&[movlb(14), movlw(0x90), movwf(RC1STA)].into_iter().chain([NOP; 340]).chain([movf_w(RC1REG), movwf(0x70), movf_w(RC1REG), movwf(0x71)]).collect::<Vec<_>>(), &[], Some(stim));
    run_to(&mut m, 360);
    assert_eq!(m.cpu.peek(0x70), b'U');
    assert_eq!(m.cpu.peek(0x71), b'V', "second read pops the second character");
    assert_eq!(pir4(&m) & RCIF, 0, "RCIF follows the FIFO: empty after both reads");
}

#[test]
fn receive_interrupt_enters_the_vector() {
    // the program starts at word 8, clear of the vector at word 4
    let body = vec![movlb(1), bsf(0x1A, 7), movlb(14), movlw(0x90), movwf(RC1STA), bsf(0x0B, 6), bsf(0x0B, 7)];
    let prog: Vec<u16> = [vec![GOTO_SELF + 8, NOP, NOP, NOP, NOP, NOP, NOP, NOP], body, vec![NOP; 400]].concat();
    let isr = [movlw(0x77), movwf(0x70), GOTO_SELF + 6];
    let stim = "[[uart]]\nport = 1\nbytes = \"U\"\nat = 20\n";
    let mut m = machine(&prog, &isr, Some(stim));
    run_to(&mut m, 150);
    assert_eq!(m.cpu.peek(0x70), 0, "not before the character is in");
    run_to(&mut m, 400);
    assert_eq!(m.cpu.peek(0x70), 0x77, "the interrupt was taken");
    let mut none = machine(&prog, &isr, None);
    run_to(&mut none, 400);
    assert_eq!(none.cpu.peek(0x70), 0, "and not without a character");
}

#[test]
fn characters_during_sleep_are_lost_and_other_sections_are_refused() {
    let prog: Vec<u16> = [vec![movlb(14), movlw(0x90), movwf(RC1STA), 0x0063], vec![NOP; 400]].concat();
    let mut m = machine(&prog, &[], Some("[[uart]]\nport = 1\nbytes = \"U\"\nat = 10\n"));
    run_to(&mut m, 400);
    assert_eq!(pir4(&m) & RCIF, 0, "the receiver has no clock in Sleep");
    let awake: Vec<u16> = prog.iter().map(|&w| if w == 0x0063 { NOP } else { w }).collect();
    let mut m = machine(&awake, &[], Some("[[uart]]\nport = 1\nbytes = \"U\"\nat = 10\n"));
    run_to(&mut m, 400);
    assert_eq!(pir4(&m) & RCIF, RCIF, "control: the same character arrives when the part is awake");
    let dev = Device::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../devices/16F17146.toml")).unwrap();
    let mut m = Machine16::new(dev).unwrap();
    assert!(m.load_stimulus("[[analog]]\npin = \"RA0\"\nvolts = 1.5\n").is_err(), "[[analog]] is not modelled on PIC16 parts yet");
    assert!(m.load_stimulus("[[uart]]\nport = 3\nbytes = \"U\"\n").is_err(), "the 16F17146 has EUSART1 and 2 only");
}

#[test]
fn txif_cannot_be_cleared_by_software() {
    // TXEN, then BCF PIR4,6 (PIR4 is at 0090h: bank 1, offset 10h): TXIF reads 1 right after and no fresh request is made
    let prog: Vec<u16> = [vec![movlb(14), movlw(0x20), movwf(TX1STA), movlb(1), 0x1000 | 6 << 7 | 0x10], vec![NOP; 40]].concat();
    let mut m = machine(&prog, &[], None);
    run_to(&mut m, 5);
    assert_eq!(pir4(&m) & TXIF, TXIF, "the clear was ignored");
}
