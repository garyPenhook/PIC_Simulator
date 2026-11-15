//! Interactive input arrives through the same timed receiver as file stimulus, after an idle run.
use devdesc::Device;
use picsim::{Machine, machine16::Machine16, periph::RxStream};
use std::path::Path;

fn device(part: &str) -> Device {
    Device::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("../../devices/{part}.toml")))
        .unwrap()
}
fn stream(at: u64) -> RxStream {
    RxStream {
        at,
        words: [u16::from(b'Z')].into(),
        bit: None,
        gap: 0,
        nine: false,
        bad_stop: false,
    }
}

#[test]
fn pic18_terminal_input_reaches_eusart_and_q41_uart() {
    for (part, q41) in [("18F27Q10", false), ("18F16Q41", true)] {
        let mut m = Machine::new(device(part)).unwrap();
        let settings: &[(&str, u8)] = if q41 {
            &[
                ("U1BRGL", 3),
                ("U1BRGH", 0),
                ("U1CON0", 0xB0),
                ("U1CON2", 0),
                ("U1CON1", 0x80),
            ]
        } else {
            &[
                ("BAUD1CON", 8),
                ("TX1STA", 4),
                ("SP1BRGL", 3),
                ("SP1BRGH", 0),
                ("RC1STA", 0x90),
            ]
        };
        for &(name, val) in settings {
            m.cpu.write(m.dev.find_sfr(name).unwrap().addr, val);
        }
        // Arm a cached idle deadline before injecting input into a lazy core.
        m.run(None, 100);
        m.cpu.lazy = true;
        m.cpu.step();
        assert!(m.cpu.p.has_uart(1));
        assert!(!m.cpu.p.has_uart(9));
        assert!(m.cpu.p.rx_send(1, stream(m.cpu.cycles + 1)));
        assert!(!m.cpu.p.rx_send(9, stream(m.cpu.cycles + 1)));
        for _ in 0..50 {
            m.cpu.step();
        }
        let flag = if q41 { "U1RXIF" } else { "RC1IF" };
        let (addr, mask) = m
            .dev
            .sfr
            .iter()
            .filter(|s| s.name.starts_with("PIR"))
            .find_map(|s| {
                s.fields
                    .iter()
                    .chain(&s.legacy)
                    .find(|f| f.name == flag)
                    .map(|f| (s.addr as usize, 1 << f.pos))
            })
            .unwrap();
        assert_ne!(
            m.cpu.m.mem[addr] & mask,
            0,
            "{part}: queued input must invalidate the cached deadline"
        );
        let name = if q41 { "U1RXB" } else { "RC1REG" };
        assert_eq!(
            m.cpu.read(m.dev.find_sfr(name).unwrap().addr),
            b'Z',
            "{part}"
        );
    }
}

#[test]
fn pic16_terminal_input_reaches_eusart_after_idle() {
    let mut m = Machine16::new(device("16F17146")).unwrap();
    // GOTO 0 keeps the CPU idle without touching any peripheral.
    m.load_hex(":020000000028D6\n:00000001FF\n").unwrap();
    for (name, val) in [
        ("BAUD1CON", 8),
        ("TX1STA", 4),
        ("SP1BRGL", 3),
        ("SP1BRGH", 0),
        ("RC1STA", 0x90),
    ] {
        m.cpu.write(m.dev.find_sfr(name).unwrap().addr, val);
    }
    m.run(None, 100);
    m.cpu.lazy = true;
    m.cpu.step();
    assert!(m.cpu.has_uart(1));
    assert!(m.cpu.rx_send(1, stream(m.cpu.cycles + 1)));
    assert!(!m.cpu.rx_send(9, stream(m.cpu.cycles + 1)));
    for _ in 0..25 {
        m.cpu.step();
    }
    assert_ne!(
        m.cpu.mem[0x90] & 0x80,
        0,
        "queued input must invalidate the cached deadline"
    );
    assert_eq!(m.cpu.read(m.dev.find_sfr("RC1REG").unwrap().addr), b'Z');
}
