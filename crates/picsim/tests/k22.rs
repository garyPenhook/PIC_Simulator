//! PIC18(L)F2x/4xK22 models (DS40001412H; `docs/FACTS-SIM.md`, "PIC18(L)F2x/4xK22"): EECON1 data EEPROM and flash, the older Timer0 and
//! Timer2, the INTCON core interrupt sources with IPEN in RCON, the ADC with ACQT, the oscillator status bits and CCP1 PWM on its fixed pin.

use devdesc::Device;
use picsim::Machine;
use std::path::Path;

fn machine(part: &str) -> Machine {
    let dev = Device::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("../../devices/{part}.toml"))).unwrap();
    let mut m = Machine::new(dev).unwrap();
    m.cpu.p.board.fosc_mhz = 4.0; // 1 000 instruction cycles per ms
    m
}

fn a(m: &Machine, n: &str) -> u32 {
    m.dev.find_sfr(n).unwrap_or_else(|| panic!("no {n}")).addr
}

/// Steps NOPs (the erased program memory is FFFFh, which runs as NOP) until cycle `t`.
fn run_to(m: &mut Machine, t: u64) {
    while m.cpu.cycles < t {
        m.cpu.step();
    }
}

fn unlock_wr(m: &mut Machine, con1: u8) {
    let (c1, c2) = (a(m, "EECON1"), a(m, "EECON2"));
    m.cpu.write(c1, con1 & !2); // WREN first: WR and WREN cannot be set by one instruction (7.4 p.101)
    m.cpu.write(c2, 0x55);
    m.cpu.write(c2, 0xAA);
    m.cpu.write(c1, con1);
}

#[test]
fn data_eeprom_write_then_read() {
    let mut m = machine("18F25K22");
    let (adr, dat, c1, pir2) = (a(&m, "EEADR"), a(&m, "EEDATA"), a(&m, "EECON1"), a(&m, "PIR2"));
    m.cpu.write(adr, 5);
    m.cpu.write(dat, 0x5A);
    let t0 = m.cpu.cycles;
    unlock_wr(&mut m, 0x06); // WREN | WR
    assert_eq!(m.cpu.read(c1) & 2, 2, "WR stays set while the byte is written");
    run_to(&mut m, t0 + 2_900);
    assert_eq!(m.cpu.read(c1) & 2, 2);
    assert_eq!(m.cpu.read(pir2) & 0x10, 0);
    run_to(&mut m, t0 + 3_100); // TDEW 3 ms typical (D175 p.430) at 4 MHz
    assert_eq!(m.cpu.read(c1) & 2, 0, "WR clears at the end");
    assert_eq!(m.cpu.read(pir2) & 0x10, 0x10, "EEIF");
    m.cpu.write(c1, 0);
    m.cpu.write(dat, 0);
    m.cpu.write(c1, 1); // RD
    assert_eq!(m.cpu.read(dat), 0x5A);
    assert_eq!(m.cpu.read(c1) & 1, 0, "RD completes at once");
}

#[test]
fn eeprom_write_needs_wren_set_before() {
    let mut m = machine("18F25K22");
    let (adr, dat, c1, c2) = (a(&m, "EEADR"), a(&m, "EEDATA"), a(&m, "EECON1"), a(&m, "EECON2"));
    m.cpu.write(adr, 9);
    m.cpu.write(dat, 0x11);
    m.cpu.write(c1, 0);
    m.cpu.write(c2, 0x55);
    m.cpu.write(c2, 0xAA);
    m.cpu.write(c1, 0x06); // WREN and WR in one write: no write (7.4)
    assert_eq!(m.cpu.read(c1) & 2, 0);
    m.cpu.write(c1, 0x04);
    m.cpu.write(c1, 0x06); // no unlock: no write
    assert_eq!(m.cpu.read(c1) & 2, 0);
}

/// TBLWT*+ of `data` from TBLPTR = `ptr` (the instruction at the reset vector).
fn tblwt(m: &mut Machine, ptr: u32, data: &[u8]) {
    for (n, sh) in [("TBLPTRL", 0), ("TBLPTRH", 8), ("TBLPTRU", 16)] {
        let r = a(m, n);
        m.cpu.write(r, (ptr >> sh) as u8);
    }
    let ta = a(m, "TABLAT");
    for d in data {
        m.cpu.prog[0] = 0x0D; // TBLWT*+
        m.cpu.prog[1] = 0x00;
        m.cpu.write(ta, *d);
        m.cpu.pc = 0;
        m.cpu.step();
    }
    m.cpu.prog[0] = 0xFF;
    m.cpu.prog[1] = 0xFF;
    m.cpu.pc = 0;
}

fn set_tblptr(m: &mut Machine, p: u32) {
    for (n, sh) in [("TBLPTRL", 0), ("TBLPTRH", 8), ("TBLPTRU", 16)] {
        let r = a(m, n);
        m.cpu.write(r, (p >> sh) as u8);
    }
}

#[test]
fn flash_block_erase_and_write_through_the_holding_registers() {
    let mut m = machine("18F25K22");
    let blk = 0x7FC0usize; // the last 64-byte block of 32 KB
    for b in &mut m.cpu.prog[blk..blk + 64] {
        *b = 0x00;
    }
    // Erase: EEPGD | WREN | FREE; the CPU stalls (TIW 2 ms) and FREE clears.
    set_tblptr(&mut m, blk as u32 + 5);
    let t0 = m.cpu.cycles;
    unlock_wr(&mut m, 0x96);
    assert!(m.cpu.prog[blk..blk + 64].iter().all(|&b| b == 0xFF), "block erased");
    assert!(m.cpu.cycles >= t0 + 2_000, "the CPU stalls for the erase");
    let c1 = a(&m, "EECON1");
    assert_eq!(m.cpu.read(c1) & 0x10, 0, "FREE clears");
    // Write two bytes; the other holding registers are FFh and leave their bytes.
    tblwt(&mut m, blk as u32 + 2, &[0x34, 0x12]);
    set_tblptr(&mut m, blk as u32 + 2);
    unlock_wr(&mut m, 0x86);
    assert_eq!(&m.cpu.prog[blk..blk + 6], &[0xFF, 0xFF, 0x34, 0x12, 0xFF, 0xFF]);
    // The holding registers are FFh after the write (6.6 note p.96): a second write of one other byte changes only that byte.
    tblwt(&mut m, blk as u32 + 10, &[0x00]);
    set_tblptr(&mut m, blk as u32 + 10);
    unlock_wr(&mut m, 0x86);
    assert_eq!(&m.cpu.prog[blk..blk + 12], &[0xFF, 0xFF, 0x34, 0x12, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x00, 0xFF]);
    let pir2 = a(&m, "PIR2");
    assert_eq!(m.cpu.read(pir2) & 0x10, 0x10, "EEIF");
}

#[test]
fn timer0_eight_bit_counts_to_ffh_and_prescales() {
    let mut m = machine("18F25K22");
    let (t0con, tl, intcon) = (a(&m, "T0CON"), a(&m, "TMR0L"), a(&m, "INTCON"));
    m.cpu.write(intcon, 0);
    m.cpu.write(tl, 0);
    m.cpu.write(t0con, 0xC0); // on, 8-bit, FOSC/4, prescaler 1:2 (PSA = 0, T0PS = 000)
    let t0 = m.cpu.cycles;
    while m.cpu.read(intcon) & 4 == 0 {
        m.cpu.step();
        assert!(m.cpu.cycles < t0 + 2_000);
    }
    let d = m.cpu.cycles - t0;
    assert!((510..=516).contains(&d), "256 counts at 1:2 is 512 cycles, took {d}");
    // 16-bit mode, no prescaler: TMR0L reads move, and TMR0H follows a TMR0L read.
    m.cpu.write(t0con, 0x88);
    let t = m.cpu.cycles + 300;
    run_to(&mut m, t);
    let th = a(&m, "TMR0H");
    let lo = m.cpu.read(tl);
    let hi = m.cpu.read(th);
    assert!(u16::from(hi) << 8 | u16::from(lo) > 250);
}

#[test]
fn timer2_older_layout_period_and_postscaler() {
    let mut m = machine("18F25K22");
    let (t2con, pr2, pir1) = (a(&m, "T2CON"), a(&m, "PR2"), a(&m, "PIR1"));
    m.cpu.write(pr2, 99);
    m.cpu.write(pir1, 0);
    m.cpu.write(t2con, 0x04 | 0x01 | 1 << 3); // on, prescaler 1:4, postscaler 1:2
    let t0 = m.cpu.cycles;
    while m.cpu.read(pir1) & 2 == 0 {
        m.cpu.step();
        assert!(m.cpu.cycles < t0 + 5_000);
    }
    let d = m.cpu.cycles - t0;
    assert!((795..=805).contains(&d), "100 counts at 1:4, twice, is 800 cycles, took {d}");
}

#[test]
fn low_priority_tmr0_interrupt_with_ipen_in_rcon() {
    let mut m = machine("18F25K22");
    let (rcon, intcon, intcon2) = (a(&m, "RCON"), a(&m, "INTCON"), a(&m, "INTCON2"));
    let r = m.cpu.read(rcon);
    m.cpu.write(rcon, r | 0x80); // IPEN
    let i2 = m.cpu.read(intcon2);
    m.cpu.write(intcon2, i2 & !4); // TMR0IP = 0: low priority
    m.cpu.write(intcon, 0x20 | 0x04 | 0x40 | 0x80); // TMR0IE, TMR0IF, GIEL, GIEH
    for _ in 0..5 {
        m.cpu.step();
    }
    assert!(m.cpu.pc >= 0x18 && m.cpu.pc < 0x30, "pc {:#x}", m.cpu.pc);
    assert_eq!(m.cpu.read(intcon) & 0x40, 0, "entry clears GIEL");
    // Without IPEN, INTCON bit 5 is TMR0IE, not IPEN: TMR0 then goes to the high vector with GIE alone (PEIE does not gate it).
    let mut m = machine("18F25K22");
    m.cpu.write(intcon, 0x20 | 0x04 | 0x80);
    for _ in 0..5 {
        m.cpu.step();
    }
    assert!(m.cpu.pc >= 0x08 && m.cpu.pc < 0x18, "pc {:#x}", m.cpu.pc);
}

#[test]
fn adc_conversion_takes_acqt_plus_eleven_tad() {
    let mut m = machine("18F25K22");
    let (c0, c2) = (a(&m, "ADCON0"), a(&m, "ADCON2"));
    m.cpu.write(c2, 0x80 | 4 << 3 | 6); // right-justified, ACQT 8 TAD, FOSC/64 (TAD = 16 cycles)
    m.cpu.write(c0, 1);
    m.cpu.write(c0, 3);
    let t0 = m.cpu.cycles;
    while m.cpu.read(c0) & 2 != 0 {
        m.cpu.step();
        assert!(m.cpu.cycles < t0 + 1_000);
    }
    assert_eq!(m.cpu.cycles - t0, 19 * 16 + 1, "(8 + 11) TAD and one cycle (Figure 17-4 p.292)");
}

#[test]
fn oscillator_status_follows_scs_and_the_pll() {
    let mut m = machine("18F25K22");
    let (con, con2, tune) = (a(&m, "OSCCON"), a(&m, "OSCCON2"), a(&m, "OSCTUNE"));
    m.cpu.write(con, 0x62); // IRCF 110, SCS = 10: the internal block
    assert_eq!(m.cpu.read(con) & 0x0C, 0x04, "HFIOFS 1, OSTS 0");
    m.cpu.write(tune, 0x40); // PLLEN, but SCS = 1x: no PLL
    assert_eq!(m.cpu.read(con2) & 0x80, 0);
    m.cpu.write(con, 0x70); // SCS = 00 with the unprogrammed FOSC = 0101 (ECHPIO6) and PLLCFG = 0
    assert_eq!(m.cpu.read(con) & 0x0C, 0x08, "OSTS 1, HFIOFS 0");
    assert_eq!(m.cpu.read(con2) & 0x80, 0, "FOSC 0000-0111 takes the PLL from PLLCFG only (Table 2-1 p.28, [A146])");
}

#[test]
fn ccp1_pwm_drives_rc2() {
    let mut m = machine("18F25K22");
    m.cpu.write(a(&m, "ANSELC"), 0);
    let (pr2, t2con, ccp1con, ccpr1l, trisc, portc) = (a(&m, "PR2"), a(&m, "T2CON"), a(&m, "CCP1CON"), a(&m, "CCPR1L"), a(&m, "TRISC"), a(&m, "PORTC"));
    m.cpu.write(pr2, 49); // 50 counts, 200 cycles at 1:1
    m.cpu.write(ccpr1l, 25); // duty 100 of 200: half
    m.cpu.write(ccp1con, 0x0C); // PWM, DC1B = 00
    m.cpu.write(trisc, 0xFB); // RC2 an output
    m.cpu.write(t2con, 0x04);
    let (mut hi, mut n) = (0, 0);
    let t0 = m.cpu.cycles;
    while m.cpu.cycles < t0 + 4_000 {
        m.cpu.step();
        n += 1;
        if m.cpu.read(portc) & 4 != 0 {
            hi += 1;
        }
    }
    assert!(hi * 10 > n * 4 && hi * 10 < n * 6, "RC2 high {hi} of {n} samples");
}
