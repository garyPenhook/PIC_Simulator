use devdesc::Device;
use picsim::{Machine, machine16::Machine16, stimulus::Stimulus};
use std::path::Path;

fn device(part: &str) -> Device {
    Device::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("../../devices/{part}.toml"))).unwrap()
}

const REPLY: &str = "[[uart_device]]\nrequest = \"AT\"\nreply = \"K\"\ndelay = 7\nbit = 4\n";

#[test]
fn uart_requests_reply_after_last_stop_bit_on_both_pic18_uarts() {
    for (part, q41) in [("18F27Q10", false), ("18F16Q41", true)] {
        let mut m = Machine::new(device(part)).unwrap();
        m.load_stimulus(&(REPLY.to_owned() + "[[uart]]\nat = 10000\nbytes = \"Z\"\n")).unwrap();
        let settings: &[(&str, u8)] = if q41 {
            &[("U1BRGL", 3), ("U1BRGH", 0), ("U1CON0", 0xB0), ("U1CON2", 0), ("U1CON1", 0x80)]
        } else {
            &[("BAUD1CON", 8), ("TX1STA", 0x24), ("SP1BRGL", 3), ("SP1BRGH", 0), ("RC1STA", 0x90)]
        };
        for &(name, val) in settings { m.cpu.write(m.dev.find_sfr(name).unwrap().addr, val); }
        let tx = m.dev.find_sfr(if q41 { "U1TXB" } else { "TX1REG" }).unwrap().addr;
        let rx = m.dev.find_sfr(if q41 { "U1RXB" } else { "RC1REG" }).unwrap().addr;
        let (flag_addr, flag_mask) = m.dev.sfr.iter().filter(|s| s.name.starts_with("PIR")).find_map(|s| {
            s.fields.iter().chain(&s.legacy).find(|f| f.name == if q41 { "U1RXIF" } else { "RC1IF" }).map(|f| (s.addr as usize, 1 << f.pos))
        }).unwrap();
        for _ in 0..2 { // the rule repeats without reloading
            let start = m.cpu.cycles;
            m.cpu.write(tx, b'A');
            m.cpu.write(tx, b'T');
            while m.cpu.cycles < start + 124 { m.cpu.step(); }
            assert_eq!(m.cpu.m.mem[flag_addr] & flag_mask, 0, "{part}: response must wait for request end + delay + RX stop sample");
            while m.cpu.cycles < start + 126 { m.cpu.step(); }
            assert_eq!(m.cpu.read(rx), b'K', "{part}");
        }
    }
}

#[test]
fn pic16_uart_device_and_clock_survive_explicit_reset() {
    let mut m = Machine16::new(device("16F17146")).unwrap();
    m.load_stimulus(&(REPLY.to_owned() + "[[clock]]\npin = \"RC0\"\nat = 10\nperiod = 10\n")).unwrap();
    for _ in 0..2 {
        for (name, val) in [("BAUD1CON", 8), ("TX1STA", 0x24), ("SP1BRGL", 3), ("SP1BRGH", 0), ("RC1STA", 0x90), ("ANSELC", 0)] {
            m.cpu.write(m.dev.find_sfr(name).unwrap().addr, val);
        }
        let tx = m.dev.find_sfr("TX1REG").unwrap().addr;
        m.cpu.write(tx, b'A');
        m.cpu.write(tx, b'T');
        while m.cpu.cycles < 120 { m.cpu.step(); }
        assert_eq!(m.cpu.read(m.dev.find_sfr("PORTC").unwrap().addr) & 1, 1);
        while m.cpu.cycles < 126 { m.cpu.step(); }
        assert_eq!(m.cpu.read(m.dev.find_sfr("RC1REG").unwrap().addr), b'K');
        assert_eq!(m.cpu.read(m.dev.find_sfr("PORTC").unwrap().addr) & 1, 0);
        m.reset().unwrap();
    }
}

#[test]
fn time_units_resolve_across_sections_and_round_up() {
    let st = Stimulus::parse(r#"
fosc_mhz = 16
[[clock]]
pin = "RB0"
at = "1 us"
period = "2.5us"
high = "750ns"
until = "1ms"
[[uart]]
bytes = "A"
at = "1ms"
bit = "250ns"
gap = "0.1 cycles"
[[pin]]
port = "B"
bit = 1
level = 1
at = "0.000002s"
[[adc]]
channel = 0
value = 123
at = "1us"
[[bounce]]
pin = "RB2"
level = 0
bounces = 1
period = ["1us", "2us"]
"#).unwrap();
    assert_eq!((st.clocks[0].at, st.clocks[0].period, st.clocks[0].high, st.clocks[0].until), (4, 10, 3, Some(4000)));
    assert_eq!((st.uart[0].1.at, st.uart[0].1.bit, st.uart[0].1.gap), (4000, Some(1), 1));
    assert_eq!(st.adc, [(0, 4, 123)]);
    assert!(st.pins.contains(&(b'B', 8, 2, 2)));
}

#[test]
fn rejects_invalid_extension_values_and_missing_hardware() {
    for text in [
        "[[clock]]\npin = 'RB0'\nperiod = 1",
        "[[clock]]\npin = 'RB0'\nperiod = 10\nhigh = 10",
        "[[clock]]\npin = 'RB0'\nperiod = 10\nat = 5\nuntil = 5",
        "[[clock]]\npin = 'RB0'\nperiod = 10\n[[clock]]\npin = 'RB0'\nperiod = 20",
        "[[pin]]\nport = 'B'\nbit = 0\nlevel = 0\nat = '1us'",
        "fosc_mhz = 0\n[[uart]]\nbytes = 'A'\nbit = '1ms'",
        "[[uart]]\nbytes = 'A'\nat = '-1cycles'",
        "[[uart]]\nbytes = 'A'\nat = 'NaN cycles'",
        "[[uart]]\nbytes = 'A'\nat = '4294967297cycles'",
        "[[uart_device]]\nrequest = ''\nreply = 'x'",
        "[[uart_device]]\nrequest = 'A'\nreply = 'x'\nbit = 0",
        "[[i2c]]\naddress = 80\nmode = 'unknown'",
        "[[i2c]]\naddress = 80\nreadonly = true",
    ] { assert!(Stimulus::parse(text).is_err(), "{text}"); }
    let mut m = Machine::new(device("18F27Q10")).unwrap();
    assert!(m.load_stimulus("[[clock]]\npin = 'RH7'\nperiod = 10").is_err());
    assert!(m.load_stimulus("[[uart_device]]\nport = 9\nrequest = 'A'\nreply = 'B'").is_err());
}

#[test]
fn aborted_uart_frame_cannot_trigger_a_device() {
    for (part, q41) in [("18F27Q10", false), ("18F16Q41", true)] {
        let mut m = Machine::new(device(part)).unwrap();
        m.load_stimulus("[[uart_device]]\nrequest = 'A'\nreply = 'K'\nbit = 4").unwrap();
        let settings: &[(&str, u8)] = if q41 {
            &[("U1BRGL", 3), ("U1BRGH", 0), ("U1CON0", 0xB0), ("U1CON1", 0x80)]
        } else {
            &[("BAUD1CON", 8), ("TX1STA", 0x24), ("SP1BRGL", 3), ("SP1BRGH", 0), ("RC1STA", 0x90)]
        };
        for &(name, val) in settings { m.cpu.write(m.dev.find_sfr(name).unwrap().addr, val); }
        let tx = m.dev.find_sfr(if q41 { "U1TXB" } else { "TX1REG" }).unwrap().addr;
        m.cpu.write(tx, b'A');
        while m.cpu.cycles < 10 { m.cpu.step(); }
        let enable = m.dev.find_sfr(if q41 { "U1CON1" } else { "TX1STA" }).unwrap().addr;
        m.cpu.write(enable, 0);
        m.cpu.write(enable, if q41 { 0x80 } else { 0x24 });
        while m.cpu.cycles < 150 { m.cpu.step(); }
        let rx = m.dev.find_sfr(if q41 { "U1RXB" } else { "RC1REG" }).unwrap().addr;
        assert_eq!(m.cpu.read(rx), 0, "{part}");
    }
}

#[test]
fn reactive_uart_rules_allow_overlapping_requests() {
    // Exercise matching through a real UART; requests AAA in AAAA overlap at the third and fourth bytes.
    let mut m = Machine::new(device("18F27Q10")).unwrap();
    m.load_stimulus("[[uart_device]]\nrequest = 'AAA'\nreply = 'K'\nbit = 1").unwrap();
    for (name, val) in [("BAUD1CON", 8), ("TX1STA", 0x24), ("SP1BRGL", 3), ("SP1BRGH", 0), ("RC1STA", 0x90)] {
        m.cpu.write(m.dev.find_sfr(name).unwrap().addr, val);
    }
    let tx = m.dev.find_sfr("TX1REG").unwrap().addr;
    for _ in 0..4 {
        m.cpu.write(tx, b'A');
        let end = m.cpu.cycles + 40;
        while m.cpu.cycles < end { m.cpu.step(); }
    }
    while m.cpu.cycles < 180 { m.cpu.step(); }
    let rx = m.dev.find_sfr("RC1REG").unwrap().addr;
    let (flag_addr, flag_mask) = m.dev.sfr.iter().filter(|s| s.name.starts_with("PIR")).find_map(|s| {
        s.fields.iter().chain(&s.legacy).find(|f| f.name == "RC1IF").map(|f| (s.addr as usize, 1 << f.pos))
    }).unwrap();
    assert_eq!(m.cpu.read(rx), b'K');
    assert_ne!(m.cpu.m.mem[flag_addr] & flag_mask, 0, "second overlapping match remains in FIFO");
    assert_eq!(m.cpu.read(rx), b'K');
    assert_eq!(m.cpu.m.mem[flag_addr] & flag_mask, 0);
    let rcsta = m.dev.find_sfr("RC1STA").unwrap().addr;
    assert_eq!(m.cpu.read(rcsta) & 2, 0, "two matches, no FIFO overrun");
}
