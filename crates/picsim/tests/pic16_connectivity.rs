//! PIC16 resolved pin connectivity, peripheral loopback, pull-ups and open-drain outputs.

use devdesc::Device;
use picsim::machine16::Machine16;
use std::path::Path;

const PARTS: [&str; 6] = ["16F17146", "16F18346", "16F13145", "16F13245", "16F15244", "16F18056"];
const NOP: u16 = 0;
const BRA_SELF: u16 = 0x33FF;

fn hexrec(b: &[u8]) -> String {
    let sum = b.iter().fold(0u8, |a, &x| a.wrapping_add(x));
    format!(":{}{:02X}\n", b.iter().map(|x| format!("{x:02X}")).collect::<String>(), sum.wrapping_neg())
}

fn machine(part: &str, prog: &[u16], stim: &str) -> Machine16 {
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
    for r in m.dev.config.clone() {
        if let Some(f) = r.fields.iter().find(|f| f.name == "WDTE") {
            put(r.addr, (r.default & !(((1u32 << f.width) - 1) << f.pos)) as u16); // the watchdog is off
        }
    }
    m.load_hex(&(text + ":00000001FF\n")).unwrap();
    m.load_stimulus(stim).unwrap();
    w(&mut m, "ANSELA", 0);
    m
}

fn a(m: &Machine16, n: &str) -> u32 {
    m.dev.find_sfr(n).unwrap_or_else(|| panic!("no {n}")).addr
}

fn r(m: &mut Machine16, n: &str) -> u8 {
    let x = a(m, n);
    m.cpu.read(x)
}

fn w(m: &mut Machine16, n: &str, v: u8) {
    let x = a(m, n);
    m.cpu.write(x, v);
}

fn run_to(m: &mut Machine16, c: u64) {
    while m.cpu.cycles < c {
        m.cpu.step();
    }
}


#[test]
fn pullups_yield_to_external_low_and_analog_masks_digital_input() {
    for part in PARTS {
        let mut m = machine(part, &[BRA_SELF], "");
        w(&mut m, "WPUA", 1);
        assert_eq!(r(&mut m, "PORTA") & 1, 1, "{part}: floating input pulled high");
        m.load_stimulus("[[pin]]\nport = \"A\"\nbit = 0\nlevel = 0\n").unwrap();
        assert_eq!(r(&mut m, "PORTA") & 1, 0, "{part}: explicit low wins");
        w(&mut m, "ANSELA", 1);
        assert_eq!(r(&mut m, "PORTA") & 1, 0, "{part}: digital buffer disabled");
    }
}

#[test]
fn open_drain_releases_high_and_sinks_low_with_gpio_and_peripheral_sources() {
    use picsim::periph::PinView;
    for part in PARTS {
        let mut m = machine(part, &[BRA_SELF], "[[pin]]\nport = \"A\"\nbit = 0\nlevel = 1\n");
        w(&mut m, "ODCONA", 1);
        w(&mut m, "TRISA", 0xFE);
        w(&mut m, "LATA", 0);
        assert_eq!(m.cpu.pin((b'A', 0)), Some((true, false)), "{part}: sinking");
        assert!(!m.cpu.open_drain((b'A', 0)));
        w(&mut m, "LATA", 1);
        assert_eq!(m.cpu.pin((b'A', 0)), Some((false, true)), "{part}: released");
        assert!(m.cpu.open_drain((b'A', 0)));
        // A WPU bit does not pull up a TRIS output, even when open drain releases it.
        let mut floating = machine(part, &[BRA_SELF], "");
        w(&mut floating, "ODCONA", 1);
        w(&mut floating, "WPUA", 1);
        w(&mut floating, "TRISA", 0xFE);
        w(&mut floating, "LATA", 1);
        assert_eq!(r(&mut floating, "PORTA") & 1, 0, "{part}: no pull-up on TRIS output");
    }
}

fn tx_code(part: &str, n: u8) -> u8 {
    (match part { "16F18346" => 0x14, "16F15244" => 5, "16F18056" => 0x0E, _ => 0x13 }) + 3 * (n - 1)
}

fn serial(m: &mut Machine16, part: &str, n: u8) {
    w(m, "ANSELA", 0);
    w(m, "RA0PPS", tx_code(part, n));
    let rx = if part == "16F18346" { "RXPPS".to_string() } else { format!("RX{n}PPS") };
    w(m, &rx, 0); // receiver observes the very pin carrying TX
    w(m, &format!("RC{n}STA"), 0x90);
    w(m, &format!("TX{n}STA"), 0x24); // four cycles per bit, TRIS deliberately left as input
}

#[test]
fn eusart_pin_waveform_loopback_and_polarity_across_families() {
    use picsim::periph::PinView;
    for part in PARTS {
        let mut m = machine(part, &[NOP; 400], "");
        serial(&mut m, part, 1);
        run_to(&mut m, 4);
        assert_eq!(m.cpu.pin((b'A', 0)), Some((true, true)), "{part}: TX automatically enables output, pps={} sta={} rc={} notes={:?}", m.cpu.peek(a(&m, "RA0PPS")), m.cpu.peek(a(&m, "TX1STA")), m.cpu.peek(a(&m, "RC1STA")), m.cpu.notes);
        w(&mut m, "TX1REG", 0xA5);
        assert_eq!(r(&mut m, "PORTA") & 1, 0, "{part}: Start bit");
        run_to(&mut m, 8);
        assert_eq!(r(&mut m, "PORTA") & 1, 1, "{part}: bit zero, LSB first");
        run_to(&mut m, 44);
        assert_eq!(r(&mut m, "RC1REG"), 0xA5, "{part}: received through PPS pin");
        w(&mut m, "BAUD1CON", 0x10);
        assert_eq!(r(&mut m, "PORTA") & 1, 0, "{part}: SCKP inverts idle");
        w(&mut m, "RC1STA", 0);
        assert!(!m.cpu.pin((b'A', 0)).unwrap().0, "{part}: SPEN releases driver");
    }
}

#[test]
fn second_eusart_has_its_own_pps_code_and_receiver() {
    for part in ["16F17146", "16F13245", "16F18056"] {
        let mut m = machine(part, &[NOP; 400], "");
        serial(&mut m, part, 2);
        run_to(&mut m, 4);
        w(&mut m, "TX2REG", 0x69);
        run_to(&mut m, 44);
        assert_eq!(r(&mut m, "RC2REG"), 0x69, "{part}");
    }
}

#[test]
fn gpio_edges_reach_timer_clock_and_ioc_without_stimulus() {
    for part in PARTS {
        let mut m = machine(part, &[NOP; 400], "");
        w(&mut m, "T0CKIPPS", 0);
        w(&mut m, "T0CON1", 0);
        w(&mut m, "T0CON0", 0x90);
        w(&mut m, "IOCAP", 1);
        w(&mut m, "TRISA", 0xFE);
        run_to(&mut m, 4);
        for at in [10, 20, 30] {
            w(&mut m, "LATA", 1);
            run_to(&mut m, at);
            w(&mut m, "LATA", 0);
            run_to(&mut m, at + 4);
        }
        assert_eq!(r(&mut m, "TMR0L"), 3, "{part}: GPIO loopback clocks Timer0");
        assert_eq!(r(&mut m, "IOCAF") & 1, 1, "{part}: output edge also reaches IOC");
    }
}

#[test]
fn pwm_edges_loop_back_to_timer0_and_ccp2_capture() {
    for part in ["16F17146", "16F18346", "16F13145", "16F13245", "16F15244"] {
        let mut m = machine(part, &[NOP; 400], "");
        w(&mut m, "TRISA", 0xFE);
        let code = match part { "16F18346" => 12, "16F15244" => 1, _ => 9 };
        w(&mut m, "RA0PPS", code);
        w(&mut m, "T0CKIPPS", 0);
        w(&mut m, "T0CON1", 0);
        w(&mut m, "T0CON0", 0x90);
        if m.dev.find_sfr("T1CLK").is_some() { w(&mut m, "T1CLK", 1); }
        w(&mut m, "T1CON", 1);
        if m.dev.find_sfr("CCPTMRS0").is_some() { w(&mut m, "CCPTMRS0", 5); }
        else if m.dev.find_sfr("CCPTMRS").is_some() { w(&mut m, "CCPTMRS", 5); }
        w(&mut m, "CCP2PPS", 0);
        w(&mut m, "CCP2CON", 0x85);
        w(&mut m, "CCPR1L", 16);
        w(&mut m, "CCP1CON", 0x8C);
        if part == "16F18346" {
            w(&mut m, "PR2", 15);
            w(&mut m, "T2CON", 4);
        } else {
            w(&mut m, "T2CLKCON", 1);
            w(&mut m, "T2PR", 15);
            w(&mut m, "T2CON", 0x80);
        }
        run_to(&mut m, 100);
        assert_eq!(r(&mut m, "TMR0L"), 6, "{part}: six PWM rising edges");
        assert_ne!(r(&mut m, "CCPR2L"), 0, "{part}: capture observes PWM edges");
    }
}

#[test]
fn uart_open_drain_uses_serial_source_and_analog_only_blocks_readback() {
    use picsim::periph::PinView;
    let mut m = machine("16F17146", &[NOP; 200], "[[pin]]\nport = \"A\"\nbit = 0\nlevel = 1\n");
    serial(&mut m, "16F17146", 1);
    w(&mut m, "ODCONA", 1);
    run_to(&mut m, 4);
    assert_eq!(m.cpu.pin((b'A', 0)), Some((false, true)));
    w(&mut m, "TX1REG", 0);
    assert_eq!(m.cpu.pin((b'A', 0)), Some((true, false)), "serial Start sinks despite LAT being zero throughout");
    run_to(&mut m, 44);
    w(&mut m, "ANSELA", 1);
    assert_eq!(r(&mut m, "PORTA") & 1, 0, "ANSEL disables the digital input");
    assert_eq!(m.cpu.pin((b'A', 0)), Some((false, true)), "physical pin remains high");
}

#[test]
fn uart_pin_receiver_reports_ninth_bit_framing_and_overrun() {
    let part = "16F17146";
    // Build three nine-bit serial frames on RA1, with a bad Stop on the first.
    let bit = 4;
    let mut stim = "[[pin]]\nport = \"A\"\nbit = 1\nlevel = 1\n".to_string();
    for (n, word) in [0x1A5u16, 0x069, 0x142].into_iter().enumerate() {
        let start = 20 + n * 48;
        for j in 0..11 {
            let level = if j == 0 { 0 } else if j <= 9 { (word >> (j - 1)) & 1 } else { u16::from(n != 0) };
            stim += &format!("[[pin]]\nport = \"A\"\nbit = 1\nlevel = {level}\nat = {}\n", start + j * bit);
        }
        stim += &format!("[[pin]]\nport = \"A\"\nbit = 1\nlevel = 1\nat = {}\n", start + 44);
    }
    let mut m = machine(part, &[NOP; 400], &stim);
    w(&mut m, "RX1PPS", 1);
    w(&mut m, "TX1STA", 4);
    w(&mut m, "RC1STA", 0xD0);
    run_to(&mut m, 165);
    assert_eq!(r(&mut m, "RC1STA") & 7, 7, "OERR, FERR and RX9D describe the overrun and first word");
    assert_eq!(r(&mut m, "RC1REG"), 0xA5);
    assert_eq!(r(&mut m, "RC1STA") & 5, 0, "second word has a good Stop and ninth bit zero");
    assert_eq!(r(&mut m, "RC1REG"), 0x69);
    w(&mut m, "RC1STA", 0xC0);
    assert_eq!(r(&mut m, "RC1STA") & 2, 0, "clearing CREN recovers from overrun");
}

#[test]
fn fastest_back_to_back_frames_keep_their_ninth_bits() {
    let part = "16F17146";
    let mut m = machine(part, &[BRA_SELF], "");
    serial(&mut m, part, 1);
    w(&mut m, "BAUD1CON", 8); // BRG16 and BRGH: one instruction clock per bit
    w(&mut m, "TX1STA", 0x65); // TX9, TXEN, BRGH, TX9D
    w(&mut m, "RC1STA", 0xD0);
    run_to(&mut m, 4);
    w(&mut m, "TX1REG", 0xA5);
    run_to(&mut m, 6);
    w(&mut m, "TX1STA", 0x64); // latch ninth bit zero for the held character
    w(&mut m, "TX1REG", 0x69);
    w(&mut m, "TX1STA", 0x65); // later TX9D changes must not change the held word
    run_to(&mut m, 28);
    assert_eq!(r(&mut m, "RC1STA") & 1, 1);
    assert_eq!(r(&mut m, "RC1REG"), 0xA5);
    assert_eq!(r(&mut m, "RC1STA") & 1, 0);
    assert_eq!(r(&mut m, "RC1REG"), 0x69);
}

#[test]
fn tx_pin_holds_its_current_bit_during_sleep() {
    use picsim::periph::PinView;
    let program: Vec<_> = [NOP; 8].into_iter().chain([0x0063]).chain([NOP; 100]).collect();
    let mut m = machine("16F17146", &program, "");
    serial(&mut m, "16F17146", 1);
    run_to(&mut m, 4);
    w(&mut m, "TX1REG", 0xA5);
    run_to(&mut m, 9);
    assert!(m.cpu.sleep_dead());
    let held = m.cpu.pin((b'A', 0));
    run_to(&mut m, 30);
    assert_eq!(m.cpu.pin((b'A', 0)), held, "the EUSART clock stops in Sleep");
}

#[test]
fn timer_loopback_includes_the_uart_start_edge_after_a_register_write() {
    let mut m = machine("16F17146", &[NOP; 200], "");
    serial(&mut m, "16F17146", 1);
    w(&mut m, "T0CKIPPS", 0);
    w(&mut m, "T0CON1", 0x20); // falling edges
    w(&mut m, "T0CON0", 0x90);
    run_to(&mut m, 4);
    // Ignore the initial change from the floating reset pin to TX idle.
    w(&mut m, "TMR0L", 0);
    w(&mut m, "TX1REG", 0); // the only falling edge is Start, immediately after this sync boundary
    run_to(&mut m, 44);
    assert_eq!(r(&mut m, "TMR0L"), 1);
}
