//! Parts on the board that hang on the PIC pins: LEDs and an SSD1306 OLED.
//!
//! * LED: on when its pin is an output (TRIS clear) at the active level. A pin that is an input leaves the LED dark
//!   (floating). The time the LED is on is counted so that a PWM or multiplexed LED shows a brightness (`duty`).
//! * OLED (SSD1306, 128 columns, 64 or 32 rows, I2C): the bytes the PIC sends to its address are decoded into the
//!   display RAM. Two ways to reach it:
//!   - `Bus::I2c(n)`: the part's hardware I2C host n (MSSP on the Q10, I2C on the Q41). The OLED answers only when
//!     the pins are wired as the code expects: SSPxCLKPPS/SSPxDATPPS (I2CxSCLPPS/I2CxSDAPPS) select the OLED's SCL and
//!     SDA pins and RxyPPS of both pins carries the bus output code (bidirectional pins, DS40002043F 18.3 p.287).
//!     Otherwise the address is not acknowledged and `status` says what is wrong.
//!   - `Bus::Gpio`: bit-banged I2C. The pins are watched after every instruction: a Start is SDA falling while SCL is
//!     high, a Stop SDA rising while SCL is high, a bit is SDA at the rising edge of SCL. The OLED pulls SDA low for the
//!     acknowledge. A pin is low when the PIC drives it (TRIS clear, LAT clear) or the OLED pulls it; otherwise it is
//!     high (external pull-ups are assumed, so an input pin reads 1 unless the OLED holds it low).
//! * SSD1306 commands: display on/off, contrast, memory addressing mode (horizontal, vertical, page), column and page
//!   range, page/column start, start line, display offset, multiplex ratio, segment remap, COM scan direction, inversion,
//!   entire display on. Accepted and ignored with a note: scrolling. Reads: the data sheet provides none in serial
//!   mode (no data read, no status read; p.17, p.33, p.43 of the PDF), so the OLED acknowledges the read address (8.1.5 a) p.19)
//!   but does not drive SDA: the host reads 0xFF, and a note says so.
//!
//! Source: SSD1306 data sheet Rev 1.1 (Solomon Systech, Apr 2008) with its charge pump application note, kept in
//! `docs/pdf/SSD1306-Rev1.1.pdf`: command table 9-1 p.28-32, I2C interface 8.1.5 p.19-21, addressing modes 10.1.3 p.34-35,
//! start line / offset / COM scan Tables 10-1..10-3 p.37-38.
//! `[A64]` What a read of the address with R/W# = 1 does is not defined beyond "read mode"; the model acknowledges and leaves SDA high.
//! The charge pump (8Dh, A[2] = 1 enables) is accepted but the display lights without it (modules with an external VCC).

use super::{I2cClient, I2cEv, Periph};
use crate::mem::DataMem;
use std::collections::BTreeSet;

const READ_NOTE: &str = "SSD1306 read: the data sheet provides no data or status read in serial mode (I2C); the OLED does not drive SDA, the host reads 0xFF";

/// (port letter, bit)
pub type Pin = (u8, u8);

/// "RB0", "B0" or "rb0".
pub fn parse_pin(s: &str) -> Option<Pin> {
    let s = s.trim().to_ascii_uppercase();
    let s = s.strip_prefix('R').unwrap_or(&s);
    let b = s.as_bytes();
    match b {
        [l @ b'A'..=b'H', d @ b'0'..=b'7'] => Some((*l, d - b'0')),
        _ => None,
    }
}

pub fn pin_name(p: Pin) -> String {
    format!("R{}{}", p.0 as char, p.1)
}

#[derive(Debug, Clone, PartialEq)]
pub struct Led {
    pub name: String,
    pub pin: Pin,
    pub active_high: bool,
    pub on: bool,
    /// (cycle, on) for every change, the last `MAX_CHANGES` of them.
    pub changes: Vec<(u64, bool)>,
    /// Instruction cycles on since the start.
    pub on_cycles: u64,
    /// Cycles on / total since the last `Board::take_window`.
    pub win_on: u64,
    pub win_total: u64,
    last: u64,
}

const MAX_CHANGES: usize = 20_000;

impl Led {
    pub fn new(name: &str, pin: Pin, active_high: bool) -> Led {
        Led { name: name.to_string(), pin, active_high, on: false, changes: Vec::new(), on_cycles: 0, win_on: 0, win_total: 0, last: 0 }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bus {
    Gpio,
    I2c(u8),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Oled {
    pub bus: Bus,
    pub scl: Pin,
    pub sda: Pin,
    pub address: u8,
    /// 64 or 32.
    pub height: u8,
    /// Why the OLED does not answer (empty when it is wired as the code expects).
    pub status: String,
    // ---- controller state
    pub on: bool,
    pub contrast: u8,
    ram: Vec<u8>,
    mode: u8,
    col: u8,
    page: u8,
    cols: (u8, u8),
    pages: (u8, u8),
    start_line: u8,
    /// Display offset (D3h) and multiplex ratio (A8h, rows 16..64).
    offset: u8,
    mux: u8,
    /// Column start register of the page addressing mode (00h-1Fh).
    col0: u8,
    remap: bool,
    com_rev: bool,
    invert: bool,
    all_on: bool,
    /// Bytes of data written to the RAM (a change counter for the viewer).
    pub writes: u64,
    // ---- bus protocol state
    in_session: bool,
    expect_ctl: bool,
    co: bool,
    dc: bool,
    pending: Option<(u8, Vec<u8>)>,
    // ---- bit-bang decoder
    last_scl: bool,
    last_sda: bool,
    phase: Phase,
    bits: u8,
    shift: u8,
    ack_phase: bool,
    ack_seen: bool,
    /// The ninth clock of a read byte: the host acknowledges (as opposed to the OLED acknowledging the address).
    host_ack: bool,
    pub pull_low: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Idle,
    Addr,
    Wr,
    Rd,
}

impl Oled {
    pub fn new(bus: Bus, scl: Pin, sda: Pin, address: u8, height: u8) -> Oled {
        Oled {
            bus,
            scl,
            sda,
            address,
            height: if height == 32 { 32 } else { 64 },
            status: String::new(),
            on: false,
            contrast: 0x7F,
            ram: vec![0; 8 * 128],
            mode: 2,
            col: 0,
            page: 0,
            cols: (0, 127),
            pages: (0, 7),
            start_line: 0,
            offset: 0,
            mux: 64,
            col0: 0,
            remap: false,
            com_rev: false,
            invert: false,
            all_on: false,
            writes: 0,
            in_session: false,
            expect_ctl: true,
            co: false,
            dc: false,
            pending: None,
            last_scl: true,
            last_sda: true,
            phase: Phase::Idle,
            bits: 0,
            shift: 0,
            ack_phase: false,
            ack_seen: false,
            host_ack: false,
            pull_low: false,
        }
    }

    /// The same OLED wiring with the controller and the decoder back at power-on.
    pub fn fresh(&self) -> Oled {
        Oled::new(self.bus, self.scl, self.sda, self.address, self.height)
    }

    /// Is pixel (x, y) lit? x 0..127, y 0..height-1.
    pub fn pixel(&self, x: usize, y: usize) -> bool {
        if !self.on {
            return false;
        }
        if self.all_on {
            return true;
        }
        // Table 10-1/10-3 (SSD1306 Rev 1.1 p.37-38): COM c shows display line (c + offset) and RAM row (line + start
        // line), both modulo the multiplex ratio; C8h scans the COMs from the other end. COMs beyond the ratio are off.
        let mux = usize::from(self.mux);
        if y >= mux {
            return false;
        }
        let com = if self.com_rev { mux - 1 - y } else { y };
        let row = (com + usize::from(self.offset) + usize::from(self.start_line)) % mux;
        let bit = self.ram[(row / 8) * 128 + x] >> (row % 8) & 1 != 0;
        bit != self.invert
    }

    /// The display as text, two pixel rows per line.
    pub fn ascii(&self) -> String {
        let h = usize::from(self.height);
        let mut s = String::new();
        for y in (0..h).step_by(2) {
            for x in 0..128 {
                s.push(match (self.pixel(x, y), self.pixel(x, y + 1)) {
                    (true, true) => '█',
                    (true, false) => '▀',
                    (false, true) => '▄',
                    _ => ' ',
                });
            }
            s.push('\n');
        }
        s
    }

    fn session_start(&mut self) {
        self.in_session = true;
        self.expect_ctl = true;
        self.co = false;
        self.pending = None;
    }

    /// A byte after the address of a write: control byte, then command or data bytes.
    pub fn byte(&mut self, b: u8, notes: &mut BTreeSet<String>) {
        if !self.in_session {
            return;
        }
        if self.expect_ctl {
            self.co = b & 0x80 != 0;
            self.dc = b & 0x40 != 0;
            self.expect_ctl = false;
            return;
        }
        if self.co {
            self.expect_ctl = true; // Co = 1: one byte, then a control byte again
        }
        if self.dc { self.data(b) } else { self.command(b, notes) }
    }

    fn data(&mut self, b: u8) {
        if usize::from(self.page) < 8 && self.col < 128 {
            // Segment remap acts on data written afterwards (the data sheet: "data already stored will have no changes").
            let seg = if self.remap { 127 - self.col } else { self.col };
            self.ram[usize::from(self.page) * 128 + usize::from(seg)] = b;
        }
        self.writes += 1;
        match self.mode {
            0 => {
                if self.col >= self.cols.1 {
                    self.col = self.cols.0;
                    self.page = if self.page >= self.pages.1 { self.pages.0 } else { self.page + 1 };
                } else {
                    self.col += 1;
                }
            }
            1 => {
                if self.page >= self.pages.1 {
                    self.page = self.pages.0;
                    self.col = if self.col >= self.cols.1 { self.cols.0 } else { self.col + 1 };
                } else {
                    self.page += 1;
                }
            }
            // Page mode: the column returns to the column start address, the page stays (10.1.3 p.34).
            _ => self.col = if self.col >= 127 { self.col0 } else { self.col + 1 },
        }
    }

    fn command(&mut self, b: u8, notes: &mut BTreeSet<String>) {
        if let Some((c, mut args)) = self.pending.take() {
            args.push(b);
            if args.len() < Self::arg_count(c) {
                self.pending = Some((c, args));
            } else {
                self.apply(c, &args, notes);
            }
            return;
        }
        match b {
            0x00..=0x0F => {
                self.col0 = self.col0 & 0xF0 | b & 15;
                self.col = self.col0;
            }
            0x10..=0x1F => {
                self.col0 = self.col0 & 15 | (b & 15) << 4;
                self.col = self.col0;
            }
            0x40..=0x7F => self.start_line = b & 0x3F,
            0xB0..=0xB7 => self.page = b & 7,
            0xA0 | 0xA1 => self.remap = b & 1 != 0,
            0xA4 | 0xA5 => self.all_on = b & 1 != 0,
            0xA6 | 0xA7 => self.invert = b & 1 != 0,
            0xAE | 0xAF => self.on = b & 1 != 0,
            0xC0 => self.com_rev = false,
            0xC8 => self.com_rev = true,
            0x2E => {}
            0x2F => {
                notes.insert("SSD1306 scrolling (2Fh) is not modelled".into());
            }
            0xE3 => {}
            c if Self::arg_count(c) > 0 => self.pending = Some((c, Vec::new())),
            _ => {}
        }
    }

    fn arg_count(c: u8) -> usize {
        match c {
            0x81 | 0x20 | 0xA8 | 0xD3 | 0xD5 | 0xD9 | 0xDA | 0xDB | 0x8D => 1,
            0x21 | 0x22 | 0xA3 => 2,
            0x29 | 0x2A => 5,
            0x26 | 0x27 => 6,
            _ => 0,
        }
    }

    fn apply(&mut self, c: u8, a: &[u8], notes: &mut BTreeSet<String>) {
        match c {
            0x81 => self.contrast = a[0],
            0x20 => self.mode = a[0] & 3,
            0x21 => {
                self.cols = (a[0] & 0x7F, a[1] & 0x7F);
                self.col = self.cols.0;
            }
            0x22 => {
                self.pages = (a[0] & 7, a[1] & 7);
                self.page = self.pages.0;
            }
            0xD3 => self.offset = a[0] & 0x3F,
            0xA8 if a[0] & 0x3F >= 15 => self.mux = (a[0] & 0x3F) + 1, // 0..14 are invalid entries
            0x26 | 0x27 | 0x29 | 0x2A | 0xA3 => {
                notes.insert("SSD1306 scroll setup is not modelled".into());
            }
            _ => {}
        }
    }

    // ---- bit-banged I2C ------------------------------------------------------------------------------------

    /// Look at the bus levels after an instruction. `scl` and `sda` are the line levels as the PIC leaves them
    /// (the OLED's own pull-down is not part of `sda`).
    fn sample(&mut self, scl: bool, sda: bool, notes: &mut BTreeSet<String>) {
        if scl && self.last_scl {
            if self.last_sda && !sda {
                self.phase = Phase::Addr; // Start or repeated Start
                self.bits = 0;
                self.shift = 0;
                self.ack_phase = false;
                self.host_ack = false;
                self.pull_low = false;
                self.in_session = false;
            } else if !self.last_sda && sda {
                self.phase = Phase::Idle; // Stop
                self.pull_low = false;
                self.in_session = false;
            }
        }
        if scl && !self.last_scl && self.phase != Phase::Idle {
            if self.bits < 8 {
                self.shift = self.shift << 1 | u8::from(sda);
                self.bits += 1;
            } else {
                self.ack_seen = sda;
            }
        }
        if !scl && self.last_scl && self.phase != Phase::Idle {
            if self.bits == 8 && !self.ack_phase {
                self.ack_phase = true;
                match self.phase {
                    Phase::Addr => {
                        if self.shift >> 1 == self.address {
                            self.phase = if self.shift & 1 != 0 { Phase::Rd } else { Phase::Wr };
                            self.pull_low = true; // acknowledge
                            if self.phase == Phase::Rd {
                                notes.insert(READ_NOTE.into());
                            }
                            if self.phase == Phase::Wr {
                                self.session_start();
                            }
                        } else {
                            self.phase = Phase::Idle;
                        }
                    }
                    Phase::Wr => {
                        let b = self.shift;
                        self.byte(b, notes);
                        self.pull_low = true;
                    }
                    _ => {
                        self.pull_low = false; // end of a read byte: the host answers
                        self.host_ack = true;
                    }
                }
            } else if self.ack_phase {
                self.ack_phase = false;
                self.bits = 0;
                self.shift = 0;
                self.pull_low = false;
                if self.phase == Phase::Rd {
                    if std::mem::take(&mut self.host_ack) && self.ack_seen {
                        self.phase = Phase::Idle; // the host did not acknowledge: end of the read
                    } else {
                        self.pull_low = false; // no read data is provided: the OLED does not drive SDA
                    }
                }
            }
        }
        self.last_scl = scl;
        self.last_sda = sda;
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Board {
    pub leds: Vec<Led>,
    pub oleds: Vec<Oled>,
    /// How much of `Periph::i2c_log` has been read.
    log_pos: usize,
}

impl Board {
    pub fn is_empty(&self) -> bool {
        self.leds.is_empty() && self.oleds.is_empty()
    }

    /// The same parts and wiring with every state back at power-on.
    pub fn fresh(&self) -> Board {
        Board {
            leds: self.leds.iter().map(|l| Led::new(&l.name, l.pin, l.active_high)).collect(),
            oleds: self.oleds.iter().map(Oled::fresh).collect(),
            log_pos: 0,
        }
    }

    /// Brightness 0..1 of each LED over the time since the last call (the last known state when no time passed).
    pub fn take_window(&mut self) -> Vec<f32> {
        self.leds
            .iter_mut()
            .map(|l| {
                let d = if l.win_total == 0 { f32::from(u8::from(l.on)) } else { l.win_on as f32 / l.win_total as f32 };
                l.win_on = 0;
                l.win_total = 0;
                d
            })
            .collect()
    }
}

impl Periph {
    /// Replace the parts on the board (their states start at power-on) and register the OLEDs on the hardware I2C buses.
    pub fn set_board(&mut self, b: Board) {
        self.board = b.fresh();
        self.board_wiring();
    }

    /// Call after the pins or addresses in `board` were edited.
    pub fn board_wiring(&mut self) {
        for s in &mut self.mssps {
            s.i2c.retain(|c| c.tag == 0);
        }
        for s in &mut self.i2cqs {
            s.i2c.retain(|c| c.tag == 0);
        }
        self.pullup = vec![0; self.ports.len()];
        self.forced = vec![0; self.ports.len()];
        for (k, o) in self.board.oleds.iter().enumerate() {
            match o.bus {
                Bus::I2c(n) => {
                    let c = I2cClient { address: o.address, fill: 0xFF, tag: k as u8 + 1, ..Default::default() }; // a read finds SDA undriven: 0xFF
                    if let Some(s) = self.mssps.iter_mut().find(|s| s.num == n) {
                        s.i2c.push(c);
                    } else if let Some(s) = self.i2cqs.iter_mut().find(|s| s.num == n) {
                        s.i2c.push(c);
                    }
                }
                Bus::Gpio => {
                    for p in [o.scl, o.sda] {
                        if let Some(i) = self.ports.iter().position(|q| q.letter == p.0) {
                            self.pullup[i] |= 1 << p.1;
                        }
                    }
                }
            }
        }
        self.board.log_pos = self.i2c_log.len();
    }

    pub fn has_port(&self, letter: u8) -> bool {
        self.ports.iter().any(|q| q.letter == letter)
    }

    /// Does the part have this pin?
    pub fn has_pin(&self, m: &DataMem, p: Pin) -> bool {
        self.ports.iter().any(|q| q.letter == p.0 && m.implemented[q.port] >> p.1 & 1 != 0)
    }

    /// Every pin of the part, for a pin chooser.
    pub fn pins(&self, m: &DataMem) -> Vec<Pin> {
        self.ports.iter().flat_map(|q| (0..8u8).filter(move |b| m.implemented[q.port] >> b & 1 != 0).map(move |b| (q.letter, b))).collect()
    }

    /// Level of a line that the PIC may pull low (open drain with an external pull-up).
    fn line_level(&self, m: &DataMem, p: Pin) -> bool {
        match self.ports.iter().find(|q| q.letter == p.0) {
            Some(q) => !(m.mem[q.tris] >> p.1 & 1 == 0 && m.mem[q.lat] >> p.1 & 1 == 0),
            None => true,
        }
    }

    fn hw_wiring(&self, m: &DataMem, o: &Oled, num: u8) -> Result<(), String> {
        let Some(pp) = self.i2c_pps.iter().find(|p| p.num == num) else {
            return Err(format!("the part has no I2C host {num}"));
        };
        for (name, inreg, inname, code, pin) in [("SCL", pp.scl_in, &pp.scl_name, pp.scl_code, o.scl), ("SDA", pp.sda_in, &pp.sda_name, pp.sda_code, o.sda)] {
            let v = m.mem[inreg];
            let sel: Pin = (b'A' + (v >> 3 & 7), v & 7);
            if sel != pin {
                return Err(format!("{inname} selects {} but the OLED {name} is on {}", pin_name(sel), pin_name(pin)));
            }
            let out = self.ppsout.iter().find(|&&(_, pi, bit)| self.ports[pi].letter == pin.0 && bit == pin.1);
            match out {
                Some(&(reg, ..)) if m.mem[reg] & 0x7F == code => {}
                Some(&(reg, ..)) => {
                    return Err(format!("{}PPS is {:#04x}, the {name} output code is {code:#04x}", pin_name(pin), m.mem[reg] & 0x7F));
                }
                None => return Err(format!("{} has no output selection (RxyPPS)", pin_name(pin))),
            }
        }
        Ok(())
    }

    /// Update the parts after an instruction ended at cycle `now`.
    pub fn board_poll(&mut self, m: &DataMem, now: u64) {
        if self.board.is_empty() {
            return;
        }
        let mut b = std::mem::take(&mut self.board);
        for l in &mut b.leds {
            let dt = now.saturating_sub(l.last);
            if l.on {
                l.on_cycles += dt;
                l.win_on += dt;
            }
            l.win_total += dt;
            l.last = now;
            let on = self.ports.iter().find(|q| q.letter == l.pin.0).is_some_and(|q| {
                let out = m.mem[q.tris] >> l.pin.1 & 1 == 0;
                let i = self.ports.iter().position(|r| r.letter == q.letter).unwrap();
                out && (self.port_levels(i, m, now) >> l.pin.1 & 1 != 0) == l.active_high
            });
            if on != l.on {
                l.on = on;
                if l.changes.len() >= MAX_CHANGES {
                    l.changes.remove(0);
                }
                l.changes.push((now, on));
            }
        }
        let mut forced = if b.oleds.is_empty() { Vec::new() } else { vec![0u8; self.ports.len()] };
        let mut notes = BTreeSet::new();
        let mut hw = Vec::new();
        for (k, o) in b.oleds.iter_mut().enumerate() {
            match o.bus {
                Bus::I2c(n) => {
                    let w = self.hw_wiring(m, o, n);
                    o.status = w.clone().err().unwrap_or_default();
                    hw.push((k, n, w.is_ok()));
                }
                Bus::Gpio => {
                    let (scl, sda) = (self.line_level(m, o.scl), self.line_level(m, o.sda));
                    o.sample(scl, sda, &mut notes);
                    if o.pull_low
                        && let Some(i) = self.ports.iter().position(|q| q.letter == o.sda.0)
                    {
                        forced[i] |= 1 << o.sda.1;
                    }
                }
            }
        }
        if !b.oleds.is_empty() {
            self.forced = forced;
        }
        // Hardware I2C: the OLED answers only when wired; bytes come from the bus log.
        for s in &mut self.mssps {
            for c in s.i2c.iter_mut().filter(|c| c.tag != 0) {
                c.absent = !hw.iter().any(|&(k, n, ok)| k + 1 == usize::from(c.tag) && n == s.num && ok);
            }
        }
        for s in &mut self.i2cqs {
            for c in s.i2c.iter_mut().filter(|c| c.tag != 0) {
                c.absent = !hw.iter().any(|&(k, n, ok)| k + 1 == usize::from(c.tag) && n == s.num && ok);
            }
        }
        while b.log_pos < self.i2c_log.len() {
            let (num, _, ev) = self.i2c_log[b.log_pos];
            b.log_pos += 1;
            for o in b.oleds.iter_mut().filter(|o| o.bus == Bus::I2c(num)) {
                match ev {
                    I2cEv::Start | I2cEv::Restart | I2cEv::Stop => o.in_session = false,
                    I2cEv::Addr(a, true) if a >> 1 == o.address => {
                        if a & 1 == 0 {
                            o.session_start();
                        } else {
                            notes.insert(READ_NOTE.into());
                        }
                    }
                    I2cEv::Addr(..) => o.in_session = false,
                    I2cEv::Write(v, true) => o.byte(v, &mut notes),
                    _ => {}
                }
            }
        }
        self.notes.extend(notes);
        self.board = b;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn oled() -> (Oled, BTreeSet<String>) {
        let mut o = Oled::new(Bus::Gpio, (b'C', 0), (b'C', 1), 0x3C, 64);
        o.session_start();
        (o, BTreeSet::new())
    }

    fn cmd(o: &mut Oled, n: &mut BTreeSet<String>, bytes: &[u8]) {
        o.byte(0x00, n);
        for &b in bytes {
            o.byte(b, n);
        }
        o.session_start();
    }

    fn data(o: &mut Oled, n: &mut BTreeSet<String>, bytes: &[u8]) {
        o.byte(0x40, n);
        for &b in bytes {
            o.byte(b, n);
        }
        o.session_start();
    }

    #[test]
    fn display_offset_and_start_line_follow_table_10_1() {
        let (mut o, mut n) = oled();
        cmd(&mut o, &mut n, &[0xAF, 0x20, 0x00]); // on, horizontal mode
        data(&mut o, &mut n, &[0x01]); // RAM row 0, column 0
        assert!(o.pixel(0, 0));
        cmd(&mut o, &mut n, &[0xD3, 8]); // offset 8: COM56 shows line 0 (Table 10-1 (b))
        assert!(!o.pixel(0, 0) && o.pixel(0, 56));
        cmd(&mut o, &mut n, &[0xD3, 0, 0x48]); // start line 8: COM56 shows RAM row 0 (Table 10-1 (c))
        assert!(o.pixel(0, 56) && !o.pixel(0, 0));
    }

    #[test]
    fn multiplex_ratio_limits_rows_and_wraps_the_ram() {
        let (mut o, mut n) = oled();
        cmd(&mut o, &mut n, &[0xAF, 0xA8, 55]); // 56 MUX
        data(&mut o, &mut n, &[0x01]);
        assert!(o.pixel(0, 0) && !o.pixel(0, 56), "COMs beyond the ratio are off");
        cmd(&mut o, &mut n, &[0xD3, 8]);
        assert!(o.pixel(0, 48), "offset 8 with 56 MUX: COM48 shows line 56 mod 56 = 0 (Table 10-1 (e))");
        cmd(&mut o, &mut n, &[0xA8, 5]); // 0..14 are invalid entries
        assert_eq!(o.mux, 56);
    }

    #[test]
    fn page_mode_column_wraps_to_the_column_start_in_the_same_page() {
        let (mut o, mut n) = oled();
        cmd(&mut o, &mut n, &[0xAF, 0xB2, 0x05, 0x10]); // page 2, column 5
        let bytes: Vec<u8> = (0..124).map(|i| i as u8 + 1).collect(); // columns 5..127 then back to 5
        data(&mut o, &mut n, &bytes);
        assert_eq!(o.ram[2 * 128 + 5], 124, "the 124th byte replaced column 5");
        assert_eq!(o.ram[3 * 128 + 5], 0, "the page did not advance");
        assert_eq!(o.ram[2 * 128 + 6], 2);
    }

    #[test]
    fn segment_remap_applies_to_data_written_afterwards() {
        let (mut o, mut n) = oled();
        cmd(&mut o, &mut n, &[0xAF, 0xB0, 0x00, 0x10]);
        data(&mut o, &mut n, &[0xFF]); // column 0 before the remap
        cmd(&mut o, &mut n, &[0xA1, 0xB0, 0x00, 0x10]);
        assert!(o.pixel(0, 0), "stored data is unchanged by A1h");
        data(&mut o, &mut n, &[0xFF]); // column 0 after: SEG127
        assert!(o.pixel(127, 0));
    }

    #[test]
    fn charge_pump_and_scroll_arguments_are_swallowed() {
        let (mut o, mut n) = oled();
        cmd(&mut o, &mut n, &[0x8D, 0x14, 0xAF, 0x26, 0, 0, 0, 7, 0, 0xFF, 0x2E]);
        assert!(o.on, "0x14 was the charge pump argument, not a column command");
        assert_eq!(o.col0, 0);
    }
}
