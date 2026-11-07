//! LEDs and an SSD1306 OLED (bit-banged, or on the MSSP's I2C host) on the pins of a PIC16 part (`Pic16::set_board`,
//! `periph/board.rs`).

use devdesc::Device;
use picsim::machine16::Machine16;
use std::path::Path;

fn machine(part: &str, stim: &str) -> Result<Machine16, String> {
    let dev = Device::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("../../devices/{part}.toml"))).unwrap();
    let mut m = Machine16::new(dev)?;
    m.load_hex(":00000001FF\n")?; // an empty program: every word is erased (a NOP-like 3FFFh is ADDLW -1, which does not matter here)
    m.load_stimulus(stim)?;
    Ok(m)
}

fn addr(m: &Machine16, n: &str) -> usize {
    m.dev.find_sfr(n).unwrap_or_else(|| panic!("no {n}")).addr as usize
}

/// Let the board look at the pins: one instruction.
fn tick(m: &mut Machine16, n: usize) {
    for _ in 0..n {
        m.cpu.pc = 0x100; // erased flash executes as ADDLW 3Fh; the PC is parked so that the run never leaves it
        m.cpu.step();
    }
}

const PARTS: [&str; 2] = ["16F17146", "16F18346"];

#[test]
fn led_follows_its_pin_and_polarity() {
    for part in PARTS {
        let mut m = machine(part, "[[led]]\npin = \"RB4\"\n[[led]]\npin = \"B5\"\nactive = \"low\"\nname = \"STATUS\"\n").unwrap();
        let (tris, lat) = (addr(&m, "TRISB"), addr(&m, "LATB"));
        tick(&mut m, 1);
        assert!(!m.cpu.board.leds[0].on && !m.cpu.board.leds[1].on, "{part}: inputs are floating: dark");
        m.cpu.mem[tris] = 0xCF;
        tick(&mut m, 1);
        assert!(!m.cpu.board.leds[0].on, "{part}: LATB4 = 0");
        assert!(m.cpu.board.leds[1].on, "{part}: an active-low LED lights with the pin low");
        m.cpu.mem[lat] = 0x10;
        tick(&mut m, 3);
        assert!(m.cpu.board.leds[0].on && m.cpu.board.leds[1].on);
        m.cpu.mem[lat] = 0x30;
        tick(&mut m, 3);
        assert!(!m.cpu.board.leds[1].on);
        assert_eq!(m.cpu.board.leds[0].changes.len(), 1);
        assert_eq!(m.cpu.board.leds[1].name, "STATUS");
    }
}

#[test]
fn led_brightness_is_the_on_time_share() {
    let mut m = machine("16F17146", "[[led]]\npin = \"RA2\"\n").unwrap();
    let (tris, lat) = (addr(&m, "TRISA"), addr(&m, "LATA"));
    m.cpu.mem[tris] = 0xFB;
    tick(&mut m, 1);
    m.cpu.board.take_window();
    for _ in 0..10 {
        m.cpu.mem[lat] = 0x04;
        tick(&mut m, 25);
        m.cpu.mem[lat] = 0;
        tick(&mut m, 75);
    }
    let d = m.cpu.board.take_window()[0];
    assert!((d - 0.25).abs() < 0.03, "duty {d}");
}

#[test]
fn a_pin_the_part_does_not_have_is_an_error() {
    assert!(machine("16F17146", "[[led]]\npin = \"RH0\"\n").is_err(), "no PORTH");
    assert!(machine("16F17146", "[[led]]\npin = \"RB0\"\n").is_err(), "the 20-pin part has RB4-RB7 only");
}

#[test]
fn an_oled_on_an_mssp_the_part_lacks_is_refused() {
    let e = machine("16F17146", "[[oled]]\nbus = \"i2c3\"\nscl = \"RC3\"\nsda = \"RC4\"\n").err().unwrap();
    assert!(e.contains("MSSP3"), "{e}");
}

const HW: &str = "[[oled]]\nbus = \"i2c1\"\nscl = \"RC3\"\nsda = \"RC4\"\n";

fn w(m: &mut Machine16, n: &str, v: u8) {
    let a = addr(m, n) as u32;
    m.cpu.write(a, v);
}

fn run_for(m: &mut Machine16, n: u64) {
    let c = m.cpu.cycles + n;
    while m.cpu.cycles < c {
        m.cpu.pc = 0x100;
        m.cpu.step();
    }
}

/// MSSP1 as the library sets it up: I2C host, SSP1ADD = 9; the pins through PPS (RC3 = 0x13, RC4 = 0x14; SCL1 = 0x1B, SDA1 = 0x1C).
fn hw(part: &str, scl_in: u8, sda_in: u8, sda_out: u8) -> Machine16 {
    let mut m = machine(part, HW).unwrap();
    w(&mut m, "SSP1ADD", 9);
    w(&mut m, "SSP1CON1", 0x28);
    w(&mut m, "SSP1CLKPPS", scl_in);
    w(&mut m, "SSP1DATPPS", sda_in);
    w(&mut m, "RC3PPS", 0x1B);
    w(&mut m, "RC4PPS", sda_out);
    run_for(&mut m, 5);
    m
}

fn hw_send(m: &mut Machine16, bytes: &[u8]) {
    w(m, "SSP1CON2", 1);
    run_for(m, 10);
    for &b in bytes {
        w(m, "SSP1BUF", b);
        run_for(m, 190);
    }
    w(m, "SSP1CON2", 4);
    run_for(m, 15);
}

#[test]
fn hardware_i2c_oled_shows_what_the_code_writes() {
    let mut m = hw("16F17146", 0x13, 0x14, 0x1C);
    hw_send(&mut m, &[0x78, 0x00, 0x8D, 0x14, 0xAF]);
    hw_send(&mut m, &[0x78, 0x40, 0xFF, 0x01]);
    let o = &m.cpu.board.oleds[0];
    assert!(o.on, "status {:?}", o.status);
    assert!((0..8).all(|y| o.pixel(0, y)), "column 0 page 0 all set");
    assert!(o.pixel(1, 0) && !o.pixel(1, 1), "second byte 0x01");
    assert_eq!(o.status, "");
}

#[test]
fn hardware_i2c_oled_ignores_a_bus_wired_to_other_pins() {
    let mut m = hw("16F17146", 0x13, 0x15, 0x1C); // SDA input on RC5
    hw_send(&mut m, &[0x78, 0x00, 0xAF]);
    let o = &m.cpu.board.oleds[0];
    assert!(!o.on);
    assert!(o.status.contains("SSP1DATPPS selects RC5"), "{}", o.status);
    let mut m = hw("16F17146", 0x13, 0x14, 0x00); // SDA output left on LATC4
    hw_send(&mut m, &[0x78, 0x00, 0xAF]);
    assert!(!m.cpu.board.oleds[0].on);
    assert!(m.cpu.board.oleds[0].status.contains("RC4PPS"), "{}", m.cpu.board.oleds[0].status);
}

#[test]
fn the_mssp_flag_is_raised_by_the_bus_and_the_stop_is_logged() {
    let mut m = hw("16F17146", 0x13, 0x14, 0x1C);
    hw_send(&mut m, &[0x78, 0x00, 0xAF]);
    assert!(m.cpu.board.oleds[0].on);
    // SSP1IF is PIR5 bit 2 on this part; reading SSP1STAT brings the module up to date and S/P show the stop
    assert_eq!(m.cpu.read(addr(&m, "SSP1STAT") as u32) & 0x18, 0x10, "P set, S clear after the stop");
}

struct Bb(Machine16, usize, usize, usize);

impl Bb {
    fn new(part: &str) -> Bb {
        let m = machine(part, "[[oled]]\nbus = \"gpio\"\nscl = \"RC0\"\nsda = \"RC1\"\naddress = 0x3C\nheight = 32\n").unwrap();
        let (t, l, p) = (addr(&m, "TRISC"), addr(&m, "LATC"), addr(&m, "PORTC"));
        let mut b = Bb(m, t, l, p);
        // ANSELC makes the bus pins analog at reset
        let ans = addr(&b.0, "ANSELC");
        b.0.cpu.mem[ans] = 0;
        b
    }
    fn set(&mut self, pin: u8, high: bool) {
        let mut t = self.0.cpu.mem[self.1];
        if high { t |= 1 << pin } else { t &= !(1 << pin) }
        self.0.cpu.mem[self.1] = t;
        self.0.cpu.mem[self.2] = 0;
        tick(&mut self.0, 2);
    }
    fn port(&self) -> u8 {
        self.0.cpu.peek(self.3 as u32)
    }
    fn start(&mut self) {
        self.set(1, true);
        self.set(0, true);
        self.set(1, false);
        self.set(0, false);
    }
    fn stop(&mut self) {
        self.set(1, false);
        self.set(0, true);
        self.set(1, true);
    }
    fn byte(&mut self, b: u8) -> bool {
        for i in (0..8).rev() {
            self.set(1, b >> i & 1 != 0);
            self.set(0, true);
            self.set(0, false);
        }
        self.set(1, true); // release for the acknowledge
        self.set(0, true);
        let ack = self.port() & 2 == 0;
        self.set(0, false);
        ack
    }
}

#[test]
fn bit_banged_oled_acknowledges_and_draws() {
    for part in PARTS {
        let mut b = Bb::new(part);
        b.start();
        assert!(b.byte(0x78), "{part}: address acknowledged");
        for c in [0x00, 0x8D, 0x14, 0xAF, 0xA1] {
            assert!(b.byte(c));
        }
        b.stop();
        b.start();
        assert!(b.byte(0x78));
        assert!(b.byte(0x40));
        assert!(b.byte(0x80));
        b.stop();
        let o = &b.0.cpu.board.oleds[0];
        assert!(o.on, "{part}");
        assert!(o.pixel(127, 7) && !o.pixel(127, 6) && !o.pixel(0, 7), "{part}: remapped: RAM column 0 shows at x = 127");
    }
}

#[test]
fn bit_banged_oled_does_not_answer_another_address() {
    let mut b = Bb::new("16F17146");
    b.start();
    assert!(!b.byte(0x7A));
}

#[test]
fn released_bus_pins_read_high_with_the_pull_ups_only_when_an_oled_is_wired() {
    let b = Bb::new("16F17146");
    assert_eq!(b.port() & 3, 3);
    let mut m = machine("16F17146", "").unwrap();
    let (t, a, p) = (addr(&m, "TRISC"), addr(&m, "ANSELC"), addr(&m, "PORTC"));
    (m.cpu.mem[t], m.cpu.mem[a]) = (0xFF, 0);
    assert_eq!(m.cpu.peek(p as u32) & 3, 0, "no OLED: inputs read 0 until driven");
}

#[test]
fn the_board_survives_a_reset_with_a_fresh_state() {
    let mut b = Bb::new("16F17146");
    b.start();
    b.byte(0x78);
    b.byte(0x00);
    b.byte(0xAF);
    b.stop();
    assert!(b.0.cpu.board.oleds[0].on);
    b.0.reset().unwrap();
    assert_eq!(b.0.cpu.board.oleds.len(), 1);
    assert!(!b.0.cpu.board.oleds[0].on);
}

#[test]
fn the_pin_list_has_every_pin_of_the_part() {
    let m = machine("16F17146", "").unwrap();
    let pins = m.cpu.pins();
    assert!(pins.contains(&(b'B', 4)) && pins.contains(&(b'C', 7)));
    assert!(!pins.contains(&(b'B', 0)) && !pins.contains(&(b'Z', 0)));
}
