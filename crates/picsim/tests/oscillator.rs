use devdesc::Device;
use picsim::{Machine, hex::Image, machine16::Machine16};
use std::path::Path;

fn dev(part: &str) -> Device {
    Device::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("../../devices/{part}.toml"))).unwrap()
}

fn config(d: &Device, fields: &[(&str, u32)]) -> Image {
    let mut img = Image::default();
    for r in &d.config {
        let mut v = r.default;
        for f in &r.fields {
            if let Some((_, value)) = fields.iter().find(|(n, _)| *n == f.name) {
                let mask = ((1 << f.width) - 1) << f.pos;
                v = v & !mask | value << f.pos;
            }
        }
        if d.device.family == "pic16e" {
            img.bytes.insert(2 * r.addr, v as u8);
            img.bytes.insert(2 * r.addr + 1, (v >> 8) as u8);
        } else {
            img.bytes.insert(r.addr, v as u8);
        }
    }
    img
}

fn w18(m: &mut Machine, reg: &str, value: u8) {
    m.cpu.write(m.dev.find_sfr(reg).unwrap().addr, value);
}
fn w16(m: &mut Machine16, reg: &str, value: u8) {
    m.cpu.write(m.dev.find_sfr(reg).unwrap().addr, value);
}

#[test]
fn pic18_reset_frequency_switch_divider_and_hold() {
    for part in ["18F27Q10", "18F16Q41"] {
        let mut m = Machine::new(dev(part)).unwrap();
        m.cpu.load(&config(&m.dev, &[("RSTOSC", 6), ("CSWEN", 1)]));
        assert_eq!(m.cpu.p.board.effective_fosc_mhz(), 1.0);
        w18(&mut m, "OSCCON1", 0x60);
        for (frq, mhz) in [1., 2., 4., 8., 12., 16., 32., 48., 64.].into_iter().enumerate() {
            w18(&mut m, "OSCFRQ", frq as u8);
            assert_eq!(m.cpu.p.board.effective_fosc_mhz(), mhz, "{part}: {frq}");
        }
        w18(&mut m, "OSCCON3", 0x80);
        w18(&mut m, "OSCCON1", 0x62);
        assert_eq!(m.cpu.p.board.effective_fosc_mhz(), 64.0);
        w18(&mut m, "OSCCON3", 0);
        assert_eq!(m.cpu.p.board.effective_fosc_mhz(), 16.0);
        w18(&mut m, "OSCCON2", 0x50); // read only
        assert_eq!(m.cpu.p.board.effective_fosc_mhz(), 16.0);
        w18(&mut m, "OSCCON1", 0x50);
        assert_eq!(m.cpu.p.board.effective_fosc_mhz(), 0.031);
        w18(&mut m, "OSCCON1", 0x40);
        assert_eq!(m.cpu.p.board.effective_fosc_mhz(), 0.032768);
        assert!(m.cpu.m.unmodelled.is_empty());
    }
}

#[test]
fn pic16_frequency_tables_and_reset_sources() {
    for part in ["16F17146", "16F13145", "16F13245", "16F15244", "16F18056"] {
        let mut m = Machine16::new(dev(part)).unwrap();
        m.cpu.load(&config(&m.dev, &[("RSTOSC", 0), ("CSWEN", 1)]));
        assert_eq!(m.cpu.board.effective_fosc_mhz(), 32.0, "{part}");
        for (frq, mhz) in [1., 2., 4., 8., 16., 32.].into_iter().enumerate() {
            w16(&mut m, "OSCFRQ", frq as u8);
            assert_eq!(m.cpu.board.effective_fosc_mhz(), mhz, "{part}: {frq}");
        }
        let rst = if part == "16F15244" { 2 } else { 6 };
        m.cpu.load(&config(&m.dev, &[("RSTOSC", rst)]));
        assert_eq!(m.cpu.board.effective_fosc_mhz(), 1.0, "{part}");
        assert!(m.cpu.unmodelled.is_empty());
    }
    let mut m = Machine16::new(dev("16F18346")).unwrap();
    m.cpu.load(&config(&m.dev, &[("RSTOSC", 0), ("CSWEN", 1)]));
    assert_eq!(m.cpu.board.effective_fosc_mhz(), 32.0); // 16 MHz and 2x PLL
    w16(&mut m, "OSCCON1", 0x60);
    for (frq, mhz) in [(0, 1.), (1, 2.), (3, 4.), (4, 8.), (5, 12.), (6, 16.), (7, 32.)] {
        w16(&mut m, "OSCFRQ", frq);
        assert_eq!(m.cpu.board.effective_fosc_mhz(), mhz);
    }
    m.cpu.load(&config(&m.dev, &[("RSTOSC", 6)]));
    assert_eq!(m.cpu.board.effective_fosc_mhz(), 1.0);
}

#[test]
fn k22_internal_clock_and_pll() {
    let mut m = Machine::new(dev("18F26K22")).unwrap();
    m.cpu.load(&config(&m.dev, &[("FOSC", 8)]));
    assert_eq!(m.cpu.p.board.effective_fosc_mhz(), 1.0);
    for (ircf, mhz) in [0.03125, 0.25, 0.5, 1., 2., 4., 8., 16.].into_iter().enumerate() {
        w18(&mut m, "OSCCON", (ircf as u8) << 4);
        assert_eq!(m.cpu.p.board.effective_fosc_mhz(), mhz);
    }
    w18(&mut m, "OSCTUNE", 0x40);
    assert_eq!(m.cpu.p.board.effective_fosc_mhz(), 64.0);
    w18(&mut m, "OSCCON", 0x72); // internal block, bypass primary PLL
    assert_eq!(m.cpu.p.board.effective_fosc_mhz(), 16.0);
    w18(&mut m, "OSCCON", 1);
    assert_eq!(m.cpu.p.board.effective_fosc_mhz(), 0.032768);
}

#[test]
fn clock_changes_preserve_elapsed_time_and_override_wins() {
    let mut m = Machine::new(dev("18F27Q10")).unwrap();
    m.cpu.load(&config(&m.dev, &[("RSTOSC", 6), ("CSWEN", 1), ("WDTE", 0)]));
    m.cpu.prog.fill(0);
    for _ in 0..10 {
        m.cpu.step();
    }
    assert_eq!(m.cpu.p.board.time_ns(10), 40_000.0);
    w18(&mut m, "OSCCON1", 0x60); // 4 MHz
    for _ in 0..10 {
        m.cpu.step();
    }
    assert_eq!(m.cpu.p.board.time_ns(20), 50_000.0);
    m.cpu.p.board.fosc_mhz = 8.0;
    w18(&mut m, "OSCFRQ", 8); // firmware selects 64 MHz, override stays 8
    assert_eq!(m.cpu.p.board.effective_fosc_mhz(), 8.0);
    for _ in 0..10 {
        m.cpu.step();
    }
    assert_eq!(m.cpu.p.board.time_ns(30), 55_000.0);
    assert_eq!(m.cpu.p.board.time_ns(10), 40_000.0);
    m.cpu.p.board.fosc_mhz = 0.0;
    m.cpu.step();
    assert_eq!(m.cpu.p.board.effective_fosc_mhz(), 64.0);
}

#[test]
fn switching_disabled_by_configuration() {
    for part in ["18F27Q10", "18F16Q41"] {
        let mut m = Machine::new(dev(part)).unwrap();
        m.cpu.load(&config(&m.dev, &[("RSTOSC", 6), ("CSWEN", 0)]));
        w18(&mut m, "OSCCON1", 0x60);
        assert_eq!(m.cpu.p.board.effective_fosc_mhz(), 1.0);
    }
}

#[test]
fn lf_timer_keeps_its_phase_across_a_clock_change() {
    let mut m = Machine::new(dev("18F27Q10")).unwrap();
    m.cpu.load(&config(&m.dev, &[("RSTOSC", 6), ("CSWEN", 1), ("WDTE", 0)]));
    m.cpu.prog.fill(0);
    w18(&mut m, "T1CLK", 4); // LFINTOSC 31 kHz
    w18(&mut m, "T1CON", 1);
    for _ in 0..100 {
        m.cpu.step();
    } // 400 us
    w18(&mut m, "OSCCON1", 0x60); // 4 MHz
    for _ in 0..100 {
        m.cpu.step();
    } // another 100 us
    let count = m.cpu.read(m.dev.find_sfr("TMR1L").unwrap().addr);
    assert_eq!(count, 15); // floor(500 us * 31 kHz)
}

#[test]
fn watchdog_keeps_elapsed_time_when_firmware_changes_fosc() {
    let fields = [("RSTOSC", 6), ("CSWEN", 1), ("WDTE", 3), ("WDTCPS", 0)];
    let mut m = Machine::new(dev("18F27Q10")).unwrap();
    m.cpu.load(&config(&m.dev, &fields));
    m.cpu.prog.fill(0);
    for _ in 0..100 {
        m.cpu.step();
    }
    w18(&mut m, "OSCCON1", 0x60);
    while m.cpu.pc != 0 && m.cpu.cycles < 1000 {
        m.cpu.step();
    }
    assert!((735..=739).contains(&m.cpu.cycles), "PIC18: {}", m.cpu.cycles);
    assert_eq!(m.cpu.p.board.effective_fosc_mhz(), 1.0, "warm reset restores RSTOSC");

    let mut m = Machine16::new(dev("16F17146")).unwrap();
    m.cpu.load(&config(&m.dev, &fields));
    m.cpu.prog.fill(0);
    for _ in 0..100 {
        m.cpu.step();
    }
    w16(&mut m, "OSCCON1", 0x60);
    while m.cpu.pc != 0 && m.cpu.cycles < 1000 {
        m.cpu.step();
    }
    assert!((735..=739).contains(&m.cpu.cycles), "PIC16: {}", m.cpu.cycles);
    assert_eq!(m.cpu.board.effective_fosc_mhz(), 1.0);
}

#[test]
fn stimulus_zero_restores_automatic_timing_with_or_without_parts() {
    for parts in ["", "[[led]]\npin = \"RC0\"\n"] {
        let mut m = Machine::new(dev("18F27Q10")).unwrap();
        m.cpu.load(&config(&m.dev, &[("RSTOSC", 6)]));
        m.load_stimulus("fosc_mhz = 8\n").unwrap();
        assert_eq!(m.cpu.p.board.effective_fosc_mhz(), 8.0);
        m.load_stimulus(&format!("fosc_mhz = 0\n{parts}")).unwrap();
        assert_eq!(m.cpu.p.board.effective_fosc_mhz(), 1.0);

        let mut m = Machine16::new(dev("16F17146")).unwrap();
        m.cpu.load(&config(&m.dev, &[("RSTOSC", 6)]));
        m.load_stimulus("fosc_mhz = 8\n").unwrap();
        assert_eq!(m.cpu.board.effective_fosc_mhz(), 8.0);
        m.load_stimulus(&format!("fosc_mhz = 0\n{parts}")).unwrap();
        assert_eq!(m.cpu.board.effective_fosc_mhz(), 1.0);
    }
}

fn r18(m: &Machine, reg: &str) -> u8 {
    m.cpu.peek(m.dev.find_sfr(reg).unwrap().addr)
}

/// `[A157]`: a switch to a source with a start-up time keeps the old clock running until the source is ready.
#[test]
fn pic18_switch_waits_for_the_pll_and_the_oscillator_start_up_timer() {
    for (part, pir, cswif) in [("18F27Q10", "PIR1", 0x40u8), ("18F16Q41", "PIR0", 0x08)] {
        // HFINTOSC 1 MHz; an HS crystal (OST 1024 TOSC) is assumed to run at the family maximum; the PLL adds 200 us.
        for (nosc, pll) in [(0x70u8, 0.0), (0x20, 200.0)] {
            let mut m = Machine::new(dev(part)).unwrap();
            m.cpu.load(&config(&m.dev, &[("RSTOSC", 6), ("CSWEN", 1), ("FEXTOSC", 2)]));
            m.load_stimulus("").ok();
            m.cpu.prog.fill(0);
            let start = m.cpu.cycles;
            w18(&mut m, "OSCCON1", nosc);
            assert_eq!(r18(&m, "OSCCON3") & 0x18, 0, "{part}: switch in progress");
            assert_eq!(r18(&m, "OSCCON2") >> 4, 6, "{part}: COSC unchanged");
            assert_eq!(m.cpu.p.board.effective_fosc_mhz(), 1.0);
            let us = 1024.0 / m.dev.fosc_max_mhz() + pll;
            let expect = (us / 4.0).ceil() as u64;
            while r18(&m, "OSCCON3") & 0x10 == 0 {
                m.cpu.step();
                assert!(m.cpu.cycles - start < 200, "{part}: never ready");
            }
            let took = m.cpu.cycles - start;
            assert!(took >= expect && took <= expect + 2, "{part}: {took} cycles, wanted {expect}");
            assert_eq!(r18(&m, "OSCCON2") >> 4, nosc >> 4);
            assert_ne!(r18(&m, pir) & cswif, 0, "{part}: CSWIF");
        }
    }
}

#[test]
fn pic18_switch_hold_abandon_and_reset() {
    let mut m = Machine::new(dev("18F27Q10")).unwrap();
    m.cpu.load(&config(&m.dev, &[("RSTOSC", 6), ("CSWEN", 1), ("FEXTOSC", 2)]));
    m.cpu.prog.fill(0);
    w18(&mut m, "OSCCON3", 0x80);
    w18(&mut m, "OSCCON1", 0x70);
    for _ in 0..100 {
        m.cpu.step();
    }
    assert_eq!(r18(&m, "OSCCON3") & 0x18, 0x08, "held: NOSCR set, ORDY clear");
    assert_ne!(r18(&m, "PIR1") & 0x40, 0);
    assert_eq!(r18(&m, "OSCCON2") >> 4, 6);
    w18(&mut m, "OSCCON3", 0x00);
    assert_eq!(r18(&m, "OSCCON3") & 0x18, 0x10);
    assert_eq!(r18(&m, "OSCCON2") >> 4, 7);
    // abandon: a request, then COSC copied back into NOSC
    w18(&mut m, "OSCCON1", 0x60);
    w18(&mut m, "OSCCON1", 0x70);
    w18(&mut m, "OSCCON1", 0x60);
    for _ in 0..100 {
        m.cpu.step();
    }
    assert_eq!(r18(&m, "OSCCON2") >> 4, 6);
    assert_eq!(r18(&m, "OSCCON3") & 0x18, 0x10);
}

#[test]
fn pic18_switch_completes_in_idle() {
    let mut m = Machine::new(dev("18F27Q10")).unwrap();
    m.cpu.load(&config(&m.dev, &[("RSTOSC", 6), ("CSWEN", 1), ("FEXTOSC", 2)]));
    m.cpu.prog.fill(0);
    m.cpu.prog[0] = 0x03; // SLEEP
    w18(&mut m, "CPUDOZE", 0x80); // IDLEN: Idle, Fosc keeps running
    w18(&mut m, "OSCCON1", 0x70);
    m.cpu.step();
    for _ in 0..50 {
        m.cpu.step();
    }
    assert_eq!(r18(&m, "OSCCON2") >> 4, 7, "Idle: the switch completes");

    // Sleep proper: it does not, until the part wakes (DS40002043F 5.3.3 p.49)
    let mut m = Machine::new(dev("18F27Q10")).unwrap();
    m.cpu.load(&config(&m.dev, &[("RSTOSC", 6), ("CSWEN", 1), ("FEXTOSC", 2), ("WDTE", 0)]));
    m.cpu.prog.fill(0);
    m.cpu.prog[0] = 0x03;
    w18(&mut m, "OSCCON1", 0x70);
    m.cpu.step();
    for _ in 0..50 {
        m.cpu.step();
    }
    assert_eq!(r18(&m, "OSCCON2") >> 4, 6, "Sleep: no switch");
}
