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
fn new_devices_and_the_parts_the_core_refuses() {
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
    // the PIC16F183xx has a 5-bit BSR and INTCON-style interrupt registers: refused, not run with the wrong decoding
    let dev = Device::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../devices/16F18346.toml")).unwrap();
    assert!(Machine16::new(dev).is_err());
}
