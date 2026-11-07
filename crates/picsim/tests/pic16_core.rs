//! The PIC16 enhanced mid-range core on a PIC16F17146 (DS40002343F): flags, cycle counts, indirect addressing, the stack,
//! interrupt entry and exit. Programs are hand-assembled 14-bit words; the opcodes are Table 41-3 (p.592-594).

use devdesc::Device;
use picsim::hex::Image;
use picsim::pic16::{C, DC, Z};
use picsim::machine16::Machine16;
use std::path::Path;

// ----- opcodes (Table 41-3) -------------------------------------------------------------------------------------
const NOP: u16 = 0x0000;
const RETURN: u16 = 0x0008;
const RETFIE: u16 = 0x0009;
const CALLW: u16 = 0x000A;
const BRW: u16 = 0x000B;
const RESET: u16 = 0x0001;
const SLEEP: u16 = 0x0063;
fn movlw(k: u16) -> u16 { 0x3000 | k }
fn addlw(k: u16) -> u16 { 0x3E00 | k }
fn sublw(k: u16) -> u16 { 0x3C00 | k }
fn movwf(f: u16) -> u16 { 0x0080 | f }
fn clrf(f: u16) -> u16 { 0x0180 | f }
fn movf(f: u16, d: u16) -> u16 { 0x0800 | d << 7 | f }
fn subwf(f: u16, d: u16) -> u16 { 0x0200 | d << 7 | f }
fn subwfb(f: u16, d: u16) -> u16 { 0x3B00 | d << 7 | f }
fn addwfc(f: u16, d: u16) -> u16 { 0x3D00 | d << 7 | f }
fn decfsz(f: u16, d: u16) -> u16 { 0x0B00 | d << 7 | f }
fn rlf(f: u16, d: u16) -> u16 { 0x0D00 | d << 7 | f }
fn lsrf(f: u16, d: u16) -> u16 { 0x3600 | d << 7 | f }
fn asrf(f: u16, d: u16) -> u16 { 0x3700 | d << 7 | f }
fn bsf(f: u16, b: u16) -> u16 { 0x1400 | b << 7 | f }
fn btfsc(f: u16, b: u16) -> u16 { 0x1800 | b << 7 | f }
fn call(k: u16) -> u16 { 0x2000 | k }
fn goto(k: u16) -> u16 { 0x2800 | k }
fn bra(k: i16) -> u16 { 0x3200 | (k as u16 & 0x1FF) }
fn retlw(k: u16) -> u16 { 0x3400 | k }
fn movlb(k: u16) -> u16 { 0x0140 | k }
fn movlp(k: u16) -> u16 { 0x3180 | k }
fn addfsr(n: u16, k: i16) -> u16 { 0x3100 | n << 6 | (k as u16 & 0x3F) }
fn moviw_k(n: u16, k: i16) -> u16 { 0x3F00 | n << 6 | (k as u16 & 0x3F) }
fn movwi_k(n: u16, k: i16) -> u16 { 0x3F80 | n << 6 | (k as u16 & 0x3F) }
fn moviw(n: u16, mm: u16) -> u16 { 0x0010 | n << 2 | mm } // mm: 0 ++FSR, 1 --FSR, 2 FSR++, 3 FSR--
fn movwi(n: u16, mm: u16) -> u16 { 0x0018 | n << 2 | mm }

/// A part with `prog` at word 0 and, when `vec` is given, `isr` at the interrupt vector (word 4). `stvren` sets the
/// STVREN configuration bit (CONFIG2 bit 12, word 8008h).
fn machine(prog: &[u16], isr: &[u16], stvren: bool) -> Machine16 {
    let dev = Device::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../devices/16F17146.toml")).unwrap();
    let mut m = Machine16::new(dev).unwrap();
    let mut img = Image::default();
    let mut put = |w: u32, v: u16| {
        img.bytes.insert(2 * w, v as u8);
        img.bytes.insert(2 * w + 1, (v >> 8) as u8);
    };
    for (i, &w) in prog.iter().enumerate() {
        put(i as u32, w);
    }
    for (i, &w) in isr.iter().enumerate() {
        put(4 + i as u32, w);
    }
    let cfg2: u16 = if stvren { 0x3FFF } else { 0x3FFF & !(1 << 12) };
    put(0x8008, cfg2);
    let mut text = String::new();
    for (&a, &b) in &img.bytes {
        let mut rec = vec![1u8, (a >> 8) as u8, a as u8, 0, b];
        if a >= 0x10000 {
            // extended linear address records are needed above 64 KiB
            let hi = (a >> 16) as u8;
            text += &hexrec(&[2, 0, 0, 4, 0, hi]);
            rec = vec![1, (a >> 8) as u8, a as u8, 0, b];
        }
        text += &hexrec(&rec);
    }
    text += ":00000001FF\n";
    m.load_hex(&text).unwrap();
    m
}

fn hexrec(b: &[u8]) -> String {
    let sum = b.iter().fold(0u8, |a, &x| a.wrapping_add(x));
    format!(":{}{:02X}\n", b.iter().map(|x| format!("{x:02X}")).collect::<String>(), sum.wrapping_neg())
}

fn run(m: &mut Machine16, n: usize) {
    for _ in 0..n {
        m.cpu.step();
    }
}

fn sfr(m: &Machine16, n: &str) -> u8 {
    m.cpu.mem[m.dev.find_sfr(n).unwrap_or_else(|| panic!("no {n}")).addr as usize]
}

fn poke(m: &mut Machine16, n: &str, v: u8) {
    m.cpu.mem[m.dev.find_sfr(n).unwrap().addr as usize] = v;
}

const W: usize = 9;
const STATUS: usize = 3;

fn flags(m: &Machine16) -> u8 {
    m.cpu.mem[STATUS] & 7
}

#[test]
fn add_and_sub_flags() {
    // ADDLW: 0FFh + 1 = 0 with C, DC, Z
    let mut m = machine(&[movlw(0xFF), addlw(1)], &[], true);
    run(&mut m, 2);
    assert_eq!((m.cpu.mem[W], flags(&m)), (0, C | DC | Z));
    // SUBLW: k - W. 3 - 5 borrows: C = 0 (p.606: C = 0 when W > k); 5 - 5 = 0 with C, DC, Z
    let mut m = machine(&[movlw(5), sublw(3), movlw(5), sublw(5)], &[], true);
    run(&mut m, 2);
    assert_eq!((m.cpu.mem[W], flags(&m) & (C | Z)), (0xFE, 0));
    run(&mut m, 2);
    assert_eq!((m.cpu.mem[W], flags(&m)), (0, C | DC | Z));
    // SUBWF f,d: f - W; 0 - 1 = FFh with no carry and no digit carry
    let mut m = machine(&[clrf(0x20), movlw(1), subwf(0x20, 1)], &[], true);
    run(&mut m, 3);
    assert_eq!((m.cpu.mem[0x20], flags(&m)), (0xFF, 0));
}

#[test]
fn carry_chain_instructions() {
    // SUBWFB: f - W - borrow, borrow = NOT C. f = 5, W = 3, C = 0 -> 1, C = 1
    let mut m = machine(&[movlw(5), movwf(0x20), movlw(3), subwfb(0x20, 0)], &[], true);
    run(&mut m, 4);
    assert_eq!((m.cpu.mem[W], flags(&m) & C), (1, C));
    // ADDWFC: W + f + C. f = 0xFF, W = 0, C = 1 (from FFh + 1) -> 0 with C and Z
    let mut m = machine(&[movlw(0xFF), movwf(0x20), addlw(1), addwfc(0x20, 0)], &[], true);
    run(&mut m, 4);
    assert_eq!((m.cpu.mem[W], flags(&m) & (C | Z)), (0, C | Z));
}

#[test]
fn shifts_and_rotates() {
    let mut m = machine(&[movlw(0x81), movwf(0x20), rlf(0x20, 0), lsrf(0x20, 0), asrf(0x20, 0)], &[], true);
    run(&mut m, 3);
    assert_eq!((m.cpu.mem[W], flags(&m) & C), (0x02, C)); // RLF through C: C was 0
    run(&mut m, 1);
    assert_eq!((m.cpu.mem[W], flags(&m) & C), (0x40, C)); // LSRF 81h
    run(&mut m, 1);
    assert_eq!((m.cpu.mem[W], flags(&m) & C), (0xC0, C)); // ASRF keeps bit 7
}

#[test]
fn status_destination_disables_flag_writes() {
    // DS40002343F 9.3 p.50: the flags an instruction sets cannot be written; CLRF STATUS leaves C and DC, sets Z.
    let mut m = machine(&[movlw(0xFF), addlw(1), clrf(3)], &[], true);
    run(&mut m, 3);
    assert_eq!(flags(&m), C | DC | Z);
    // MOVWF STATUS (no flags set by the instruction) writes C, DC, Z but not TO and PD
    let mut m = machine(&[movlw(0x00), movwf(3)], &[], true);
    let before = m.cpu.mem[STATUS];
    run(&mut m, 2);
    assert_eq!(m.cpu.mem[STATUS] & 0x18, before & 0x18);
    assert_eq!(m.cpu.mem[STATUS] & 7, 0);
}

#[test]
fn cycle_counts() {
    // 1 cycle: MOVLW. 2: GOTO, CALL, RETURN, BRA, a taken skip. DECFSZ not taken is 1.
    let mut m = machine(&[movlw(2), movwf(0x20), decfsz(0x20, 1), goto(5), NOP, call(8), NOP, NOP, RETURN], &[], true);
    let cy = |m: &mut Machine16| m.cpu.step();
    assert_eq!(cy(&mut m), 1);
    assert_eq!(cy(&mut m), 1);
    assert_eq!(cy(&mut m), 1); // DECFSZ -> 1, not zero: no skip
    assert_eq!(cy(&mut m), 2); // GOTO 5
    assert_eq!(cy(&mut m), 2); // CALL 8
    assert_eq!(cy(&mut m), 2); // RETURN
    assert_eq!(m.cpu.pc, 6);
    let mut m = machine(&[movlw(1), movwf(0x20), decfsz(0x20, 1), NOP], &[], true);
    run(&mut m, 2);
    assert_eq!(m.cpu.step(), 2); // DECFSZ -> 0: skip, 2 cycles
    assert_eq!(m.cpu.pc, 4);
    let mut m = machine(&[bra(2), NOP, NOP, NOP], &[], true);
    assert_eq!((m.cpu.step(), m.cpu.pc), (2, 3)); // BRA: PC + 1 + k
}

#[test]
fn branches_and_computed_calls() {
    // BRW: PC + 1 + W; CALLW: PCLATH:W
    let mut m = machine(&[movlw(2), BRW, NOP, NOP, NOP], &[], true);
    run(&mut m, 2);
    assert_eq!(m.cpu.pc, 4); // BRW at 1 -> 1 + 1 + 2
    let mut m = machine(&[movlp(0), movlw(7), CALLW], &[], true);
    run(&mut m, 3);
    assert_eq!((m.cpu.pc, m.cpu.depth(), m.cpu.tos()), (7, 1, 3));
    // GOTO / CALL take PC[14:11] from PCLATH[6:3]
    let mut m = machine(&[movlp(0x08), goto(5)], &[], true);
    run(&mut m, 2);
    assert_eq!(m.cpu.pc, 0x805); // PCLATH = 08h -> PCLATH[6:3] = 1 -> PC[14:11] = 1
}

#[test]
fn retlw_returns_a_literal() {
    let mut m = machine(&[call(3), NOP, NOP, retlw(0x5A)], &[], true);
    run(&mut m, 2);
    assert_eq!((m.cpu.mem[W], m.cpu.pc, m.cpu.depth()), (0x5A, 1, 0));
}

#[test]
fn common_ram_and_core_registers_are_the_same_in_every_bank() {
    // Common RAM 70h-7Fh and the core registers appear in every bank (9.2 p.42)
    let mut m = machine(&[movlw(0x77), movlb(5), movwf(0x70), movlb(0), movf(0x70, 0), movlb(9), movlw(0x12), movwf(0x0A)], &[], true);
    run(&mut m, 5);
    assert_eq!(m.cpu.mem[0x70], 0x77);
    assert_eq!(m.cpu.mem[W], 0x77);
    run(&mut m, 3);
    assert_eq!(sfr(&m, "PCLATH"), 0x12); // PCLATH is at offset 0Ah of bank 9, same register
}

#[test]
fn indirect_addressing_banked_linear_and_flash() {
    // Banked: FSR0 = 0x0120 is bank 2, offset 20h (absolute 0x120)
    let mut m = machine(
        &[movlw(0x20), movwf(4), movlw(0x01), movwf(5), movlw(0xA5), movwf(0), movf(0, 0)],
        &[],
        true,
    );
    run(&mut m, 7);
    assert_eq!((m.cpu.mem[0x120], m.cpu.mem[W]), (0xA5, 0xA5));
    // Linear: 2000h + 80 is the first GPR byte of bank 1 (absolute 0xA0), 2000h + 160 that of bank 2 (0x120)
    let mut m = machine(
        &[movlw(80), movwf(4), movlw(0x20), movwf(5), movlw(0x3C), movwi_k(0, 0), movlw(160), movwf(4), movlw(0x3C), movwi_k(0, 0), movlw(0), moviw_k(0, 0)],
        &[],
        true,
    );
    run(&mut m, 12);
    assert_eq!((m.cpu.mem[0xA0], m.cpu.mem[0x120], m.cpu.mem[W]), (0x3C, 0x3C, 0x3C));
}

#[test]
fn flash_read_through_fsr_returns_low_byte_of_the_word() {
    // word 5 = MOVLW 0x34 = 0x3034; FSR0 = 8005h; MOVIW 0[FSR0] -> 34h
    let mut m = machine(&[movlw(5), movwf(4), movlw(0x80), movwf(5), moviw_k(0, 0), movlw(0x34)], &[], true);
    run(&mut m, 5);
    assert_eq!(m.cpu.mem[W], 0x34);
    // writes to flash through INDF are ignored
    let mut m = machine(&[movlw(100), movwf(4), movlw(0x80), movwf(5), movlw(0x11), movwi_k(0, 0), NOP], &[], true);
    run(&mut m, 6);
    assert_eq!(m.cpu.fetch(100), 0x3FFF);
}

#[test]
fn moviw_and_movwi_modes() {
    // ++FSR0, FSR0++, --FSR0, FSR0-- (p.601, p.603)
    let mut m = machine(
        &[movlw(0x30), movwf(4), clrf(5), movlw(0xAA), movwi(0, 2), movlw(0xBB), movwi(0, 0), moviw(0, 1), moviw(0, 3)],
        &[],
        true,
    );
    run(&mut m, 5);
    assert_eq!((m.cpu.mem[0x30], sfr(&m, "FSR0L")), (0xAA, 0x31)); // FSR0++ writes at 30h then increments
    run(&mut m, 2);
    assert_eq!((m.cpu.mem[0x32], sfr(&m, "FSR0L")), (0xBB, 0x32)); // ++FSR0 increments, then writes at 32h
    run(&mut m, 1);
    assert_eq!((m.cpu.mem[W], sfr(&m, "FSR0L")), (0x00, 0x31)); // --FSR0 decrements to 31h first, which holds 00
}

#[test]
fn indirect_pointing_at_indf_reads_zero() {
    // FSR0 = 0 points at INDF0 itself: reads 0, writes are ignored (9.6 p.55)
    let mut m = machine(&[movlw(0x55), movwf(0), movf(0, 1)], &[], true);
    run(&mut m, 3);
    assert_eq!(m.cpu.mem[0], 0);
    assert_eq!(flags(&m) & Z, Z);
}

#[test]
fn stack_overflow_without_reset_wraps() {
    // 17th CALL: STKOVF set, pointer 10h, entry 0 overwritten (Fig. 9-17 p.55)
    let mut m = machine(&[call(0)], &[], false);
    run(&mut m, 16);
    assert_eq!(m.cpu.depth(), 16);
    assert_eq!(sfr(&m, "PCON0") & 0x80, 0);
    run(&mut m, 1);
    assert_eq!(sfr(&m, "PCON0") & 0x80, 0x80);
    assert_eq!(sfr(&m, "STKPTR"), 0x10);
}

#[test]
fn stack_overflow_with_stvren_resets() {
    let mut m = machine(&[call(0)], &[], true);
    for _ in 0..16 {
        m.cpu.step();
        assert_eq!(sfr(&m, "PCON0") & 0x80, 0);
    }
    m.cpu.step(); // the 17th push
    assert_eq!((m.cpu.pc, sfr(&m, "STKPTR"), sfr(&m, "PCON0") & 0x80), (0, 0x1F, 0x80));
}

#[test]
fn stack_underflow_with_stvren_resets() {
    let mut m = machine(&[NOP, RETURN], &[], true);
    run(&mut m, 2);
    assert_eq!((m.cpu.pc, sfr(&m, "PCON0") & 0x40), (0, 0x40));
}

#[test]
fn reset_instruction_clears_nri() {
    let mut m = machine(&[NOP, RESET], &[], true);
    run(&mut m, 2);
    assert_eq!(m.cpu.pc, 0);
    assert_eq!(sfr(&m, "PCON0") & 0x04, 0);
}

#[test]
fn tos_is_readable_and_writable() {
    // CALL pushes pc+1 = 1; TOSL/TOSH show it, a write changes the return address
    let mut m = machine(&[call(3), NOP, NOP, NOP], &[], true);
    run(&mut m, 1);
    let a = m.dev.find_sfr("TOSL").unwrap().addr;
    assert_eq!(m.peek(a, 2), vec![1, 0]);
    // a write to TOSL (bank 63, offset 6Eh) changes the return address
    let mut m = machine(&[call(3), NOP, NOP, movlb(63), movlw(9), movwf(0x6E), RETURN], &[], true);
    run(&mut m, 5);
    assert_eq!(m.cpu.pc, 9);
}

#[test]
fn addfsr_is_signed_and_carries_into_the_high_byte() {
    let mut m = machine(&[movlw(0x40), movwf(4), addfsr(0, -3), addfsr(0, 31)], &[], true);
    run(&mut m, 4);
    assert_eq!(sfr(&m, "FSR0L"), 0x5C);
    let mut m = machine(&[movlw(0xFF), movwf(6), addfsr(1, 1)], &[], true);
    run(&mut m, 3);
    assert_eq!((sfr(&m, "FSR1H"), sfr(&m, "FSR1L")), (1, 0));
}

#[test]
fn interrupt_saves_and_restores_context() {
    // TMR0IF & TMR0IE (PIR0/PIE0 bit 5) with GIE: no PEIE needed for group 0 (Fig. 12-1 p.110)
    let isr = [movlw(0xEE), clrf(4), RETFIE];
    let mut m = machine(&[movlw(0x42), NOP, NOP], &isr, true);
    run(&mut m, 1);
    poke(&mut m, "PIE0", 0x20);
    poke(&mut m, "PIR0", 0x20);
    poke(&mut m, "FSR0L", 0x99);
    poke(&mut m, "INTCON", 0x80);
    let c0 = m.cpu.cycles;
    m.cpu.step(); // takes the interrupt instead of the NOP
    assert_eq!((m.cpu.pc, m.cpu.cycles - c0, sfr(&m, "INTCON") & 0x80), (4, 2, 0));
    assert_eq!((sfr(&m, "WREG_SHAD"), sfr(&m, "FSR0L_SHAD"), m.cpu.depth(), m.cpu.tos()), (0x42, 0x99, 1, 1));
    run(&mut m, 3); // MOVLW, CLRF FSR0L, RETFIE
    assert_eq!((m.cpu.pc, sfr(&m, "WREG"), sfr(&m, "FSR0L"), sfr(&m, "INTCON") & 0x80), (1, 0x42, 0x99, 0x80));
    assert_eq!(m.cpu.depth(), 0);
}

#[test]
fn other_groups_need_peie() {
    // PIE1/PIR1 (any bit) only interrupts with PEIE set
    let mut m = machine(&[NOP, NOP, NOP], &[NOP], true);
    poke(&mut m, "PIE1", 0x01);
    poke(&mut m, "PIR1", 0x01);
    poke(&mut m, "INTCON", 0x80);
    m.cpu.step();
    assert_eq!(m.cpu.pc, 1);
    poke(&mut m, "INTCON", 0xC0);
    m.cpu.step();
    assert_eq!(m.cpu.pc, 4);
}

#[test]
fn sleep_waits_for_an_enabled_flag_and_runs_one_more_instruction() {
    let mut m = machine(&[SLEEP, NOP, NOP], &[NOP], true);
    run(&mut m, 1);
    assert!(m.cpu.sleep_dead());
    assert_eq!(sfr(&m, "STATUS") & 0x18, 0x10); // PD = 0, TO = 1
    poke(&mut m, "PIE0", 0x20);
    poke(&mut m, "PIR0", 0x20);
    poke(&mut m, "INTCON", 0x80);
    assert!(!m.cpu.sleep_dead());
    m.cpu.step(); // the instruction after SLEEP runs before the interrupt (12.7 p.113)
    assert_eq!(m.cpu.pc, 2);
    m.cpu.step();
    assert_eq!(m.cpu.pc, 4);
}

#[test]
fn btfsc_tests_a_bit_and_skips() {
    let mut m = machine(&[bsf(0x20, 3), btfsc(0x20, 3), movlw(1), btfsc(0x20, 4), movlw(2), NOP], &[], true);
    run(&mut m, 2); // BTFSC bit set: no skip
    assert_eq!(m.cpu.pc, 2);
    run(&mut m, 2); // MOVLW 1, BTFSC bit clear: skips MOVLW 2
    assert_eq!((m.cpu.mem[W], m.cpu.pc), (1, 5));
    run(&mut m, 1);
    assert_eq!(m.cpu.mem[W], 1); // MOVLW 2 never ran
}

#[test]
fn pcl_write_loads_pc_from_pclath() {
    // MOVWF PCL: PC[14:8] = PCLATH, PC[7:0] = W (9.4.1 p.51), two cycles
    let mut m = machine(&[movlp(0x01), movlw(0x10), movwf(2)], &[], true);
    run(&mut m, 2);
    assert_eq!(m.cpu.step(), 2);
    assert_eq!(m.cpu.pc, 0x110);
}

#[test]
fn shifts_leave_dc_alone() {
    // LSLF, LSRF and ASRF affect only C and Z (Table 41-3 p.592): DC set by ADDLW 0Fh to W = 1 survives
    for sh in [lsrf(0x20, 0), asrf(0x20, 0), 0x3500 | 0x20] {
        let mut m = machine(&[movlw(1), addlw(0x0F), movlw(2), movwf(0x20), sh], &[], true);
        run(&mut m, 5);
        assert_eq!(flags(&m) & DC, DC, "opcode {sh:#06x}");
    }
}

#[test]
fn flash_pointer_read_modify_write_costs_one_extra_cycle() {
    // BSF INDF0,3 with FSR0 in program flash: 1 + 1 cycles, not 1 + 2 (p.591 note 2); the write itself is ignored
    let mut m = machine(&[movlw(100), movwf(4), movlw(0x80), movwf(5), bsf(0, 3)], &[], true);
    run(&mut m, 4);
    assert_eq!(m.cpu.step(), 2);
}

#[test]
fn reset_reloads_everything_loaded() {
    let mut m = machine(&[movlw(0x11)], &[], true);
    m.load_hex(&(hexrec(&[2, 0, 2, 0, 0x22, 0x3E]) + ":00000001FF\n")).unwrap(); // word 1 = 3E22h (ADDLW 22h)
    run(&mut m, 2);
    assert_eq!(m.cpu.mem[W], 0x33);
    m.reset().unwrap();
    assert_eq!((m.cpu.pc, m.cpu.fetch(0), m.cpu.fetch(1)), (0, movlw(0x11), addlw(0x22)));
}

#[test]
fn every_modelled_pic16_part_runs() {
    for part in ["16F17126", "16F17146", "16F18056", "16F18076", "16F13145", "16F15244"] {
        let dev = Device::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("../../devices/{part}.toml"))).unwrap();
        let mut m = Machine16::new(dev).unwrap_or_else(|e| panic!("{part}: {e}"));
        m.load_hex(&{
            let w = [movlw(0xFF), addlw(1), movlw(7), movwf(0x70)];
            let mut t = String::new();
            for (i, v) in w.iter().enumerate() {
                t += &hexrec(&[2, 0, 2 * i as u8, 0, *v as u8, (*v >> 8) as u8]);
            }
            t + ":00000001FF\n"
        })
        .unwrap();
        run(&mut m, 4);
        assert_eq!((m.cpu.mem[0x70], flags(&m)), (7, C | DC | Z), "{part}");
    }
    // every PIC16F183xx member: the 5-bit-BSR core (MOVLB 31 is 003Fh); the shadow registers are in bank 31
    for part in ["18313", "18323", "18324", "18325", "18326", "18344", "18345", "18346"] {
        for pre in ["16F", "16LF"] {
            let part = format!("{pre}{part}");
            let mut m = machine_on(&part, &[movlb5(31), movlw(0x11), movwf(0x65), movlw(0xFF), addlw(1), movlw(7), movwf(0x70)], &[], true);
            run(&mut m, 7);
            assert!(m.cpu.bsr5, "{part}");
            assert_eq!((sfr(&m, "BSR"), sfr(&m, "WREG_SHAD"), m.cpu.mem[0x70], flags(&m)), (31, 0x11, 7, C | DC | Z), "{part}");
        }
    }
}

// ----- PIC16F183xx (DS40001839E): 5-bit BSR, FSR map, older Timer1/Timer2 -------------------------------------------

fn movlb5(k: u16) -> u16 { 0x0020 | k } // Table 34-3 p.399, p.406

/// `machine` on another part.
fn machine_on(part: &str, prog: &[u16], isr: &[u16], stvren: bool) -> Machine16 {
    let dev = Device::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("../../devices/{part}.toml"))).unwrap();
    let mut m = Machine16::new(dev).unwrap_or_else(|e| panic!("{part}: {e}"));
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
    put(0x8008, if stvren { 0x3FFF } else { 0x3FFF & !(1 << 12) });
    m.load_hex(&(text + ":00000001FF\n")).unwrap();
    m
}

#[test]
fn f183_movlb_is_five_bits_and_the_six_bit_encoding_is_undefined() {
    let mut m = machine_on("16F18346", &[movlb5(5), movlb(3)], &[], true);
    run(&mut m, 2);
    assert_eq!(sfr(&m, "BSR"), 5); // 0143h is not MOVLB on this part
    assert!(m.cpu.notes.iter().any(|n| n.contains("undefined opcode 0x0143")));
    assert_eq!((m.cpu.disasm(movlb5(31)), m.cpu.disasm(movlb(3))), ("movlb   31".to_string(), "dw      0x0143".to_string()));
    // and on a 6-bit part 0020h-003Fh are not MOVLB
    assert_eq!(picsim::pic16::disasm(movlb5(5)), "dw      0x0025");
}

#[test]
fn f183_fsr_map_linear_reserved_and_eeprom() {
    // DS40001839E Fig. 4-8 p.60, 4.5.2-4.5.3 p.62: linear 2000h-29AFh = banks 0-30, 1000h-1FFFh and 29B0h.. reserved
    // (read 0, writes ignored), EEPROM at 7000h-70FFh readable with one extra cycle.
    let fsr1 = |v: u16| [movlw(v >> 8), movwf(0x07), movlw(v & 0xFF), movwf(0x06)];
    let mut prog = Vec::new();
    for (at, val) in [(0x2050u16, 0x5A), (0x29AF, 0x6B), (0x29B0, 0x7C), (0x1020, 0x8D)] {
        prog.extend(fsr1(at));
        prog.extend([movlw(val), movwi_k(1, 0)]);
    }
    let mut m = machine_on("16F18346", &prog, &[], true);
    run(&mut m, prog.len());
    assert_eq!((m.cpu.mem[0xA0], m.cpu.mem[30 * 128 + 0x20 + 79]), (0x5A, 0x6B));
    // nothing behind the reserved ranges (as linear memory 29B0h would be bank 31's 0FA0h)
    assert_eq!((m.cpu.mem[0x1020], m.cpu.mem[0xFA0]), (0, 0));
    // EEPROM byte 5 = A5h in the hex (word F005h, byte address 1E00Ah)
    let mut m = machine_on("16F18346", &[movlw(0x70), movwf(0x05), movlw(5), movwf(0x04), moviw_k(0, 0), moviw_k(0, 1)], &[], true);
    m.load_hex(&(hexrec(&[2, 0, 0, 4, 0, 1]) + &hexrec(&[1, 0xE0, 0x0A, 0, 0xA5]) + ":00000001FF\n")).unwrap();
    run(&mut m, 4);
    let c = m.cpu.cycles;
    m.cpu.step();
    assert_eq!((m.cpu.mem[W], m.cpu.cycles - c), (0xA5, 2));
    m.cpu.step();
    assert_eq!(m.cpu.mem[W], 0xFF); // erased
}

#[test]
fn f183_timer1_clock_in_t1con_start_delay_and_tmr1h_write_clears_the_prescaler() {
    // TMR1L/H 19h/1Ah, T1CON 1Bh (bank 0). T1CON = 01h: TMR1CS = 00 (Fosc/4), 1:1, ON, written at cycle 2. The first
    // clock is counted at cycle 4 (MDB, tests/probe/t1h_prescaler.S, [A110]); a TMR1L read sees its own cycle ([A2]).
    let mut m = machine_on("16F18346", &[movlw(0x01), movwf(0x1B), NOP, NOP, movf(0x19, 0)], &[], true);
    run(&mut m, 5);
    assert_eq!(m.cpu.mem[W], 2);
    // TMR1CS = 01 (Fosc): four ticks a cycle
    let mut m = machine_on("16F18346", &[movlw(0x41), movwf(0x1B), NOP, NOP, movf(0x19, 0)], &[], true);
    run(&mut m, 5);
    assert_eq!(m.cpu.mem[W], 8);
    // 1:8 prescaler, running from cycle 1 (T1CON set directly): the prescaler holds 6 ticks after the CLRF TMR1H of
    // cycle 6, which clears it (27.3 p.286, MDB). A read at cycle 8 (2 ticks later) still sees 0, where 8 ticks
    // without the clear would have made 1; the read at cycle 14 (8 ticks after the clear) sees 1.
    let mut prog = vec![NOP; 5];
    prog.extend([clrf(0x1A), NOP, movf(0x19, 0), NOP, NOP, NOP, NOP, NOP, movf(0x19, 0)]);
    let mut m = machine_on("16F18346", &prog, &[], true);
    poke(&mut m, "T1CON", 0x31);
    run(&mut m, 8);
    assert_eq!((m.cpu.cycles, m.cpu.mem[W]), (8, 0));
    run(&mut m, 6);
    assert_eq!((m.cpu.cycles, m.cpu.mem[W]), (14, 1));
}

#[test]
fn f183_timer2_older_layout_period_prescaler_and_flag_timing() {
    // TMR2/PR2/T2CON 1Dh/1Eh/1Fh, TMR2IF = PIR1 bit 1 (11h). T2CON = 04h (ON, 1:1, 1:1) written at cycle 4: the first
    // clock is at cycle 6 ([A110]), TMR2 = 3 = PR2 at cycle 8, back to 0 with TMR2IF at cycle 9 (MDB,
    // tests/probe/t2_f183.S).
    let mut prog = vec![movlw(3), movwf(0x1E), movlw(0x04), movwf(0x1F)];
    prog.extend([NOP; 12]);
    let mut m = machine_on("16F18346", &prog, &[], true);
    run(&mut m, 8);
    assert_eq!((sfr(&m, "TMR2"), sfr(&m, "PIR1") & 2), (3, 0));
    run(&mut m, 1);
    assert_eq!((sfr(&m, "TMR2"), sfr(&m, "PIR1") & 2), (0, 2));
    // 1:4 prescaler (CKPS = 01), 1:2 postscaler (OUTPS = 0001): T2CON = 0Dh, period (1 + 1) x 4 x 2 = 16 cycles
    let mut prog = vec![movlw(1), movwf(0x1E), movlw(0x0D), movwf(0x1F)];
    prog.extend([NOP; 40]);
    let mut m = machine_on("16F18346", &prog, &[], true);
    run(&mut m, 4 + 16);
    assert_eq!(sfr(&m, "PIR1") & 2, 0);
    run(&mut m, 1);
    assert_eq!(sfr(&m, "PIR1") & 2, 2);
    // a BCF of the flag in the cycle it is set wins (MDB, CLR = 1); one cycle later the flag was set first
    for (at, left) in [(9usize, 0u8), (10, 0), (8, 2)] {
        let mut prog = vec![movlw(3), movwf(0x1E), movlw(0x04), movwf(0x1F)];
        prog.extend(vec![NOP; at - 5]);
        prog.push(0x1000 | 1 << 7 | 0x11); // BCF PIR1,1 at cycle `at`
        prog.extend([NOP; 4]);
        let mut m = machine_on("16F18346", &prog, &[], true);
        run(&mut m, at);
        assert_eq!(m.cpu.cycles, at as u64);
        run(&mut m, if at == 8 { 1 } else { 0 });
        assert_eq!(sfr(&m, "PIR1") & 2, left, "BCF at cycle {at}");
    }
}

// ----- Timer0 (DS40002343F ch.22 p.244-249) ---------------------------------------------------------------------

fn t0(m: &mut Machine16, con0: u8, con1: u8, h: u8) {
    poke(m, "T0CON1", con1);
    poke(m, "TMR0H", h);
    poke(m, "T0CON0", con0);
}

#[test]
fn timer0_8bit_period_is_tmr0h_plus_one_clocks() {
    let mut m = machine(&[NOP; 16], &[], true);
    t0(&mut m, 0x80, 0x40, 3); // EN, 8-bit, Fosc/4, 1:1, period 4
    run(&mut m, 3);
    assert_eq!((sfr(&m, "PIR0") & 0x20, sfr(&m, "TMR0L")), (0, 3));
    run(&mut m, 1);
    assert_eq!((sfr(&m, "PIR0") & 0x20, sfr(&m, "TMR0L"), sfr(&m, "T0CON0") & 0x20), (0x20, 0, 0x20)); // flag, back to 0, OUT toggled
    // prescaler 1:4 and postscaler 1:2: a flag every 2 x 4 x 4 cycles
    let mut m = machine(&[NOP; 40], &[], true);
    t0(&mut m, 0x81, 0x42, 3);
    run(&mut m, 31);
    assert_eq!(sfr(&m, "PIR0") & 0x20, 0);
    run(&mut m, 1);
    assert_eq!(sfr(&m, "PIR0") & 0x20, 0x20);
}

#[test]
fn timer0_16bit_rolls_over_and_buffers_tmr0h() {
    // TMR0H = FFh, TMR0L = FEh through the registers; MD16 buffers TMR0H on a TMR0L write and read
    let prog = [movlb(3), movlw(0xFF), movwf(0x1D), movlw(0xFE), movwf(0x1C), NOP, NOP, NOP, movf(0x1C, 0)];
    let mut m = machine(&prog, &[], true);
    poke(&mut m, "T0CON1", 0x40);
    poke(&mut m, "T0CON0", 0x90); // EN, 16-bit
    run(&mut m, 5); // TMR0L written: count = FFFEh, counting starts after that instruction
    assert_eq!((sfr(&m, "TMR0L"), sfr(&m, "PIR0") & 0x20), (0xFE, 0));
    run(&mut m, 2);
    assert_eq!(sfr(&m, "PIR0") & 0x20, 0x20); // FFFF, then 0000: roll-over
    run(&mut m, 2); // NOP; MOVF TMR0L,W reads 02h (count through the reading cycle) and loads TMR0H with the high byte
    assert_eq!((m.cpu.mem[W], sfr(&m, "TMR0H")), (2, 0));
}

#[test]
fn timer0_interrupt_enters_the_vector() {
    let mut m = machine(&[NOP; 8], &[NOP], true);
    t0(&mut m, 0x80, 0x40, 1); // period 2
    poke(&mut m, "PIE0", 0x20);
    poke(&mut m, "INTCON", 0x80);
    run(&mut m, 2); // flag set at the end of the second NOP
    assert_eq!(m.cpu.pc, 2);
    let c = m.cpu.cycles;
    m.cpu.step(); // [A108] the NOP already fetched runs first, then the entry: one step, as MDB's STEPI
    assert_eq!((m.cpu.pc, m.cpu.cycles - c, m.cpu.tos()), (4, 3, 3));
}

#[test]
fn a_request_made_pending_by_software_is_taken_at_once() {
    // [A104] MDB (tests/probe/sw_pending.S): BSF PIR0 or BSF PIE0 with GIE set enters right after the BSF
    for (pir, pie) in [(0x0C, 0x16), (0x16, 0x0C)] {
        let prog = [movlb(1), bsf(pie, 5), bsf(0x0B, 7), bsf(pir, 5), NOP, NOP];
        let mut m = machine(&prog, &[NOP], true);
        run(&mut m, 4);
        let c = m.cpu.cycles;
        m.cpu.step();
        assert_eq!((m.cpu.pc, m.cpu.cycles - c, m.cpu.tos()), (4, 2, 4));
    }
}

#[test]
fn a_flag_cleared_in_the_cycle_a_timer_sets_it_stays_clear_and_the_next_set_is_a_fresh_request() {
    // Woken from Sleep with GIE set, the instruction after SLEEP (BCF PIR0,TMR0IF) runs first (13.2.2 p.139). Timer0
    // (period 1) sets the flag in that same cycle; the clear wins (MDB on the PIC16F18346's Timer2,
    // tests/probe/t2_f183.S CLR = 1, [A110]). Timer0 sets it again at the end of the NOP: a fresh request ([A108]), so
    // the instruction already fetched after the NOP (word 4) runs before the entry and the stacked PC is 5.
    let bcf_tmr0if = 0x1000 | 5 << 7 | 0x0C;
    let mut m = machine(&[movlb(1), SLEEP, bcf_tmr0if, NOP], &[NOP], true);
    t0(&mut m, 0x80, 0x40, 0);
    run(&mut m, 2);
    poke(&mut m, "PIE0", 0x20);
    poke(&mut m, "PIR0", 0x20);
    poke(&mut m, "INTCON", 0x80);
    m.cpu.step(); // wakes and runs the BCF
    assert_eq!((m.cpu.pc, sfr(&m, "PIR0") & 0x20), (3, 0));
    m.cpu.step(); // the NOP; the flag is set again at its end, no entry yet
    assert_eq!((m.cpu.pc, m.cpu.depth(), sfr(&m, "PIR0") & 0x20), (4, 0, 0x20));
    let c = m.cpu.cycles;
    m.cpu.step(); // word 4, then the entry
    assert_eq!((m.cpu.pc, m.cpu.cycles - c, m.cpu.tos()), (4, 3, 5));
}

#[test]
fn a_breakpoint_at_the_stacked_pc_is_not_hit_before_the_entry() {
    // MDB (tests/probe/int_first.S, CHAIN=1 PERIOD=10): with breakpoints at the stacked PC and at the vector, the
    // vector is hit first
    let mut m = machine(&[NOP; 8], &[NOP], true);
    t0(&mut m, 0x80, 0x40, 1); // period 2: the flag is set at the end of the second NOP
    poke(&mut m, "PIE0", 0x20);
    poke(&mut m, "INTCON", 0x80);
    m.breakpoints.extend([3, 4]);
    assert_eq!(m.run(None, 100), picsim::Stop::Breakpoint(4));
    assert_eq!((m.cpu.tos(), m.cpu.cycles), (3, 5));
}

#[test]
fn sleep_fetched_as_a_request_arrives_executes_as_a_nop() {
    // 13.2.2 p.139: an enabled flag set before SLEEP executes makes it a NOP (TO, PD unchanged). MDB, SLEEP as the
    // instruction already fetched when TMR0IF rises with GIE set: vector 3 cycles after SLEEP started, PD = 1.
    let mut m = machine(&[NOP, NOP, SLEEP, NOP], &[NOP], true);
    t0(&mut m, 0x80, 0x40, 1); // the flag is set at the end of the second NOP
    poke(&mut m, "PIE0", 0x20);
    poke(&mut m, "INTCON", 0x80);
    run(&mut m, 2);
    let c = m.cpu.cycles;
    m.cpu.step();
    assert_eq!((m.cpu.pc, m.cpu.cycles - c, m.cpu.tos(), sfr(&m, "STATUS") & 0x18), (4, 3, 3, 0x18));
    m.cpu.step(); // runs the ISR: the part is not asleep
    assert_eq!(m.cpu.pc, 5);
    // with GIE clear the same holds, and execution goes on after SLEEP
    let mut m = machine(&[SLEEP, NOP], &[], true);
    poke(&mut m, "PIE0", 0x20);
    poke(&mut m, "PIR0", 0x20);
    m.cpu.step();
    assert!(!m.cpu.sleep_dead());
    assert_eq!((sfr(&m, "STATUS") & 0x18, m.cpu.step(), m.cpu.pc), (0x18, 1, 2));
}

#[test]
fn a_fresh_request_whose_instruction_clears_gie_is_dropped() {
    let bcf_gie = 0x1000 | 7 << 7 | 0x0B;
    let mut m = machine(&[NOP, NOP, bcf_gie, NOP], &[NOP], true);
    t0(&mut m, 0x80, 0x40, 1);
    poke(&mut m, "PIE0", 0x20);
    poke(&mut m, "INTCON", 0x80);
    run(&mut m, 2);
    m.cpu.step(); // BCF INTCON,GIE: no entry follows
    assert_eq!((m.cpu.pc, m.cpu.depth(), sfr(&m, "PIR0") & 0x20), (3, 0, 0x20));
}

#[test]
fn timer0_stops_in_sleep_and_other_clocks_are_noted() {
    let mut m = machine(&[SLEEP, NOP], &[], true);
    t0(&mut m, 0x80, 0x40, 0xFF);
    run(&mut m, 1);
    let n = sfr(&m, "TMR0L");
    for _ in 0..20 {
        m.cpu.step();
    }
    assert_eq!(sfr(&m, "TMR0L"), n); // Fosc/4 does not run in Sleep
    let mut m = machine(&[NOP, NOP], &[], true);
    t0(&mut m, 0x80, 0xA0, 0xFF); // MFINTOSC
    run(&mut m, 2);
    assert!(m.cpu.notes.iter().any(|n| n.contains("Timer0")));
}

// ----- Timer1 (DS40002343F ch.23 p.252-267) and Timer2 (ch.24 p.268-287) ----------------------------------------

// TMR1L/H = 30Ch/30Dh (bank 6), T2TMR/T2PR/T2CON = 38Ch/38Dh/38Eh (bank 7); TMR1IF is PIR1 bit 6, TMR2IF PIR2 bit 2.

#[test]
fn timer1_counts_through_the_prescaler_and_rolls_over() {
    // TMR1H = FFh, TMR1L = FDh through the registers; counting resumes after the TMR1L write
    let prog = [movlb(6), movlw(0xFF), movwf(0x0D), movlw(0xFD), movwf(0x0C), NOP, NOP, NOP, NOP];
    let mut m = machine(&prog, &[], true);
    poke(&mut m, "T1CLK", 1); // Fosc/4
    poke(&mut m, "T1CON", 0x01);
    run(&mut m, 7);
    assert_eq!((sfr(&m, "TMR1H"), sfr(&m, "TMR1L"), sfr(&m, "PIR1") & 0x40), (0xFF, 0xFF, 0));
    run(&mut m, 1);
    assert_eq!((sfr(&m, "TMR1H"), sfr(&m, "TMR1L"), sfr(&m, "PIR1") & 0x40), (0, 0, 0x40)); // FFFF -> 0000: TMR1IF
    // prescaler 1:2 halves, Fosc (CS = 2) is four ticks a cycle
    let mut m = machine(&[NOP; 16], &[], true);
    poke(&mut m, "T1CLK", 1);
    poke(&mut m, "T1CON", 0x11);
    run(&mut m, 9);
    assert_eq!(sfr(&m, "TMR1L"), 4);
    let mut m = machine(&[NOP; 16], &[], true);
    poke(&mut m, "T1CLK", 2);
    poke(&mut m, "T1CON", 0x01);
    run(&mut m, 2);
    assert_eq!(sfr(&m, "TMR1L"), 8);
}

#[test]
fn timer1_rd16_buffers_tmr1h() {
    // RD16 + 1:8 prescaler: TMR1H (buffer) then TMR1L write 1234h; a TMR1L read returns 34h and loads the buffer with 12h
    let prog = [movlb(6), movlw(0x12), movwf(0x0D), movlw(0x34), movwf(0x0C), NOP, movf(0x0C, 0), NOP, NOP];
    let mut m = machine(&prog, &[], true);
    poke(&mut m, "T1CLK", 1);
    poke(&mut m, "T1CON", 0x33);
    run(&mut m, 7);
    assert_eq!((m.cpu.mem[W], sfr(&m, "TMR1H")), (0x34, 0x12));
}

#[test]
fn timer1_starts_one_cycle_after_on_and_a_tmr1h_write_clears_the_prescaler_without_rd16() {
    // MDB on the PIC16F17146 (tests/probe/t1h_prescaler.S NEWT1 = 1, [A110]). T1CON = 01h written at cycle 3: the first
    // Fosc/4 clock is counted at cycle 5, so a TMR1L read at cycle 6 sees 2.
    let mut m = machine(&[movlb(6), movlw(0x01), movwf(0x0E), NOP, NOP, movf(0x0C, 0)], &[], true);
    poke(&mut m, "T1CLK", 1);
    run(&mut m, 6);
    assert_eq!(m.cpu.mem[W], 2);
    // RD16 = 0, 1:8, running from cycle 1: CLRF TMR1H at cycle 7 clears the prescaler (DS40002343F 23.3 p.255), so
    // the read at cycle 9 still sees 0 (9 ticks without the clear would have made 1) and the one at cycle 15 sees 1
    let mut prog = vec![movlb(6), NOP, NOP, NOP, NOP, NOP];
    prog.extend([clrf(0x0D), NOP, movf(0x0C, 0), NOP, NOP, NOP, NOP, NOP, movf(0x0C, 0)]);
    let mut m = machine(&prog, &[], true);
    poke(&mut m, "T1CLK", 1);
    poke(&mut m, "T1CON", 0x31);
    run(&mut m, 9);
    assert_eq!((m.cpu.cycles, m.cpu.mem[W]), (9, 0));
    run(&mut m, 6);
    assert_eq!((m.cpu.cycles, m.cpu.mem[W]), (15, 1));
}

#[test]
fn timer1_gate_and_unknown_clocks_are_noted_and_it_stops_in_sleep() {
    let mut m = machine(&[NOP; 4], &[], true);
    poke(&mut m, "T1CLK", 1);
    poke(&mut m, "T1GCON", 0x80);
    poke(&mut m, "T1CON", 0x01);
    run(&mut m, 3);
    assert_eq!(sfr(&m, "TMR1L"), 0);
    assert!(m.cpu.notes.iter().any(|n| n.contains("Timer1 gate")));
    let mut m = machine(&[NOP; 4], &[], true);
    poke(&mut m, "T1CLK", 8); // SOSC
    poke(&mut m, "T1CON", 0x01);
    run(&mut m, 3);
    assert!(m.cpu.notes.iter().any(|n| n.contains("Timer1 clock source")));
    let mut m = machine(&[SLEEP, NOP], &[], true);
    poke(&mut m, "T1CLK", 1);
    poke(&mut m, "T1CON", 0x01);
    run(&mut m, 1);
    let n = sfr(&m, "TMR1L");
    run(&mut m, 20);
    assert_eq!(sfr(&m, "TMR1L"), n);
}

fn t2(m: &mut Machine16, con: u8, hlt: u8, pr: u8) {
    poke(m, "T2CLKCON", 1);
    poke(m, "T2HLT", hlt);
    poke(m, "T2PR", pr);
    poke(m, "T2CON", con);
}

#[test]
fn timer2_period_is_pr_plus_one_clocks_with_prescaler_and_postscaler() {
    let mut m = machine(&[NOP; 40], &[], true);
    t2(&mut m, 0x80, 0, 3);
    run(&mut m, 3);
    assert_eq!((sfr(&m, "T2TMR"), sfr(&m, "PIR2") & 4), (3, 0));
    run(&mut m, 1);
    assert_eq!((sfr(&m, "T2TMR"), sfr(&m, "PIR2") & 4), (0, 4)); // matched at 3, back to 0 on the next clock
    // 1:4 prescaler, 1:2 postscaler: a flag every 2 x 4 x 4 cycles
    let mut m = machine(&[NOP; 40], &[], true);
    t2(&mut m, 0xA1, 0, 3);
    run(&mut m, 31);
    assert_eq!(sfr(&m, "PIR2") & 4, 0);
    run(&mut m, 1);
    assert_eq!(sfr(&m, "PIR2") & 4, 4);
}

#[test]
fn timer2_one_shot_stops_at_the_match_and_con_write_keeps_t2tmr() {
    let mut m = machine(&[NOP; 16], &[], true);
    t2(&mut m, 0x80, 0x08, 2); // software-start one-shot
    run(&mut m, 2);
    assert_eq!((sfr(&m, "T2TMR"), sfr(&m, "T2CON") & 0x80), (2, 0x80));
    run(&mut m, 1);
    assert_eq!((sfr(&m, "T2TMR"), sfr(&m, "T2CON") & 0x80, sfr(&m, "PIR2") & 4), (0, 0, 4));
    run(&mut m, 5);
    assert_eq!(sfr(&m, "T2TMR"), 0); // stays stopped
    // a T2CON write does not clear T2TMR; a T2TMR write sets it
    let prog = [movlb(7), NOP, NOP, movlw(0x80), movwf(0x0E), NOP, movlw(5), movwf(0x0C), NOP];
    let mut m = machine(&prog, &[], true);
    t2(&mut m, 0x80, 0, 0xFF);
    run(&mut m, 5);
    assert_eq!(sfr(&m, "T2TMR"), 5); // cycles 1..5 counted; the T2CON write at cycle 5 left the count
    run(&mut m, 4);
    assert_eq!(sfr(&m, "T2TMR"), 6); // 5 written by MOVWF at cycle 8, one more tick in cycle 9
}

#[test]
fn timer2_interrupt_and_reset() {
    let mut m = machine(&[NOP; 8], &[NOP], true);
    t2(&mut m, 0x80, 0, 1);
    poke(&mut m, "PIE2", 0x04);
    poke(&mut m, "INTCON", 0xC0); // GIE, PEIE
    run(&mut m, 2);
    assert_eq!(m.cpu.pc, 2);
    let c = m.cpu.cycles;
    m.cpu.step(); // [A108] the NOP already fetched, then the entry
    assert_eq!((m.cpu.pc, m.cpu.cycles - c, m.cpu.tos()), (4, 3, 3));
    // RESET: Timer2 clears T2TMR (MCLR column 0), Timer1 keeps its count (u)
    let prog = [movlb(6), movlw(5), movwf(0x0C), movlb(7), movwf(0x0C), RESET];
    let mut m = machine(&prog, &[], true);
    run(&mut m, 6);
    assert_eq!((m.cpu.pc, sfr(&m, "TMR1L"), sfr(&m, "T2TMR")), (0, 5, 0));
}
