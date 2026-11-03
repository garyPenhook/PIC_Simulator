//! Stimulus file: what the outside world does to the part over time (TOML).
//!
//! ```toml
//! [[uart]]            # characters sent to the receiver of an EUSART
//! port = 1            # EUSART number, default 1
//! at = 5000           # instruction cycle the first Start bit falls
//! bytes = "Hello\r"   # a string, or  data = [0x48, 0x165]  (values up to 0x1FF with nine = true)
//! bit = 139           # sender's bit time in instruction cycles; default: the receiver's own rate
//! gap = 0             # idle cycles between characters, default 0
//! nine = false        # 11-bit characters (the ninth data bit is bit 8 of the value)
//! bad_stop = false    # send the Stop bit as 0 (framing error)
//!
//! [[adc]]             # the voltage on an ADC input, as a conversion code (0 .. 2^bits - 1)
//! channel = 13        # ADPCH value
//! value = 512
//! at = 0              # from this instruction cycle on, default 0; later entries replace earlier ones
//!
//! [[i2c]]            # a client on the I2C bus of MSSP `port` (Q10): ACKs `address` (7 bits), sends `data` then `fill` when read
//! port = 1            # MSSP number, default 1
//! address = 0x50
//! data = [0x12, 0x34]
//! fill = 0xFF         # default 0xFF
//!
//! [[spi]]            # the client on an SPI host (Q41 SPI, Q10 MSSP): answers each exchanged byte with the next of `data`
//! port = 1            # SPI number, default 1
//! data = [0x12, 0x34] # bytes the client sends back, in order
//! fill = 0            # sent after `data` is used up, default 0
//!
//! [[pin]]             # one input pin of a port
//! port = "B"          # port letter
//! bit = 5             # 0-7
//! level = 1           # 0 or 1 (also true/false); from `at` on, later entries replace earlier ones
//! at = 0
//!
//! [[port]]            # several pins at once
//! port = "B"
//! value = 0x21        # levels of the pins in `mask`
//! mask = 0xFF         # default 0xFF
//! at = 0
//!
//! fosc_mhz = 64       # optional: the PIC clock, for the OLED's scrolling time (default 64)
//!
//! [[led]]             # an LED on a pin: on while the pin is an output at the active level
//! pin = "RB0"         # the PIC pin the LED is wired to
//! active = "high"     # "high" (default) or "low" (the LED is wired to Vdd through the pin)
//! name = "LED1"       # optional
//!
//! [[oled]]            # an SSD1306 OLED (I2C)
//! bus = "i2c1"        # "i2c1"/"i2c2": the part's hardware I2C host; "gpio": bit-banged on the pins below
//! scl = "RC3"         # the PIC pins the OLED is wired to; the code must use the same ones
//! sda = "RC4"         #   (hardware bus: SSPxCLKPPS/SSPxDATPPS and RxyPPS of both pins; see periph/board.rs)
//! address = 0x3C      # 7 bits, default 0x3C
//! height = 64         # 64 (default) or 32
//! external_vcc = false  # true: the module has its own VCC and lights without the charge pump (8Dh 14h)
//! ```
//! Pins read 0 until driven. Only input pins (TRISx = 1) with ANSELx = 0 show the level; an output pin shows its latch.

use crate::periph::{parse_pin, Bus, I2cClient, Led, Oled, RxStream, SpiClient};
use devdesc::Device;

/// Largest `at`, `bit` and `gap`: 2^32 instruction cycles. Keeps the receiver's cycle arithmetic far from u64 overflow.
pub const MAX_CYCLES: u64 = 1 << 32;

#[derive(Debug, Default, PartialEq)]
pub struct Stimulus {
    pub uart: Vec<(u8, RxStream)>,
    /// (SPI number, client)
    pub spi: Vec<(u8, SpiClient)>,
    /// (MSSP number, client)
    pub i2c: Vec<(u8, I2cClient)>,
    /// (channel, from cycle, code)
    pub adc: Vec<(u8, u64, u16)>,
    /// (port letter, from cycle, pin mask, levels)
    pub pins: Vec<(u8, u64, u8, u8)>,
    /// LEDs and OLEDs on the board.
    pub board: crate::periph::Board,
}

impl Stimulus {
    pub fn parse(text: &str) -> Result<Stimulus, String> {
        let doc: toml::Table = text.parse().map_err(|e: toml::de::Error| e.to_string())?;
        let mut st = Stimulus::default();
        for (k, v) in &doc {
            if k == "pin" || k == "port" {
                for e in v.as_array().ok_or(format!("{k} must be [[{k}]] entries"))? {
                    let t = e.as_table().ok_or(format!("{k} entry must be a table"))?;
                    let keys: &[&str] = if k == "pin" { &["port", "bit", "level", "at"] } else { &["port", "value", "mask", "at"] };
                    for key in t.keys() {
                        if !keys.contains(&key.as_str()) {
                            return Err(format!("{k}: unknown key {key}"));
                        }
                    }
                    let letter = match t.get("port").and_then(|x| x.as_str()).map(str::as_bytes) {
                        Some([c @ b'A'..=b'H']) => *c,
                        _ => return Err(format!("{k}.port must be a letter A to H")),
                    };
                    let int = |n: &str, max: u64| -> Result<Option<u64>, String> {
                        t.get(n).map_or(Ok(None), |x| {
                            x.as_integer().and_then(|i| u64::try_from(i).ok()).filter(|i| *i <= max).map(Some).ok_or(format!("{k}.{n} must be an integer 0..{max}"))
                        })
                    };
                    let at = int("at", MAX_CYCLES)?.unwrap_or(0);
                    if k == "pin" {
                        let bit = int("bit", 7)?.ok_or("pin needs bit")? as u8;
                        let level = match t.get("level") {
                            Some(toml::Value::Boolean(b)) => u8::from(*b),
                            Some(x) if matches!(x.as_integer(), Some(0 | 1)) => x.as_integer().unwrap() as u8,
                            _ => return Err("pin.level must be 0, 1, true or false".into()),
                        };
                        st.pins.push((letter, at, 1 << bit, level << bit));
                    } else {
                        let mask = int("mask", 0xFF)?.unwrap_or(0xFF) as u8;
                        st.pins.push((letter, at, mask, int("value", 0xFF)?.ok_or("port needs value")? as u8));
                    }
                }
                continue;
            }
            if k == "fosc_mhz" {
                let f = v.as_float().or_else(|| v.as_integer().map(|i| i as f64)).filter(|f| *f > 0.0 && *f <= 64.0);
                st.board.fosc_mhz = f.ok_or("fosc_mhz must be a number in 0..64")?;
                continue;
            }
            if k == "led" || k == "oled" {
                for e in v.as_array().ok_or(format!("{k} must be [[{k}]] entries"))? {
                    let t = e.as_table().ok_or(format!("{k} entry must be a table"))?;
                    let keys: &[&str] = if k == "led" { &["pin", "active", "name"] } else { &["bus", "scl", "sda", "address", "height", "external_vcc"] };
                    for key in t.keys() {
                        if !keys.contains(&key.as_str()) {
                            return Err(format!("{k}: unknown key {key}"));
                        }
                    }
                    let pin = |n: &str| -> Result<_, String> {
                        let s = t.get(n).and_then(|x| x.as_str()).ok_or(format!("{k} needs {n} (a pin such as \"RB0\")"))?;
                        parse_pin(s).ok_or(format!("{k}.{n} = {s:?} is not a pin (RA0..RH7)"))
                    };
                    if k == "led" {
                        let active_high = match t.get("active").map(|x| x.as_str()) {
                            None | Some(Some("high")) => true,
                            Some(Some("low")) => false,
                            _ => return Err("led.active must be \"high\" or \"low\"".into()),
                        };
                        let p = pin("pin")?;
                        let name = t.get("name").and_then(|x| x.as_str()).map_or_else(|| format!("LED{}", st.board.leds.len() + 1), String::from);
                        st.board.leds.push(Led::new(&name, p, active_high));
                    } else {
                        let bus = match t.get("bus").and_then(|x| x.as_str()) {
                            Some("gpio") => Bus::Gpio,
                            Some(b) if b.starts_with("i2c") => Bus::I2c(b[3..].parse().map_err(|_| format!("oled.bus {b:?}: i2c1 or i2c2"))?),
                            _ => return Err("oled.bus must be \"i2c1\", \"i2c2\" or \"gpio\"".into()),
                        };
                        let int = |n: &str, d: i64, max: i64| -> Result<u8, String> {
                            let v = t.get(n).map_or(Ok(d), |x| x.as_integer().ok_or(format!("oled.{n} must be an integer")))?;
                            if (0..=max).contains(&v) { Ok(v as u8) } else { Err(format!("oled.{n} must be 0..{max}")) }
                        };
                        let height = int("height", 64, 64)?;
                        if height != 64 && height != 32 {
                            return Err("oled.height must be 64 or 32".into());
                        }
                        let mut o = Oled::new(bus, pin("scl")?, pin("sda")?, int("address", 0x3C, 0x7F)?, height);
                        o.external_vcc = match t.get("external_vcc") {
                            None => false,
                            Some(x) => x.as_bool().ok_or("oled.external_vcc must be true or false")?,
                        };
                        st.board.oleds.push(o);
                    }
                }
                continue;
            }
            if k == "spi" {
                for e in v.as_array().ok_or("spi must be [[spi]] entries")? {
                    let t = e.as_table().ok_or("spi entry must be a table")?;
                    for k in t.keys() {
                        if !["port", "data", "fill"].contains(&k.as_str()) {
                            return Err(format!("spi: unknown key {k}"));
                        }
                    }
                    let byte = |x: &toml::Value| x.as_integer().and_then(|i| u8::try_from(i).ok()).ok_or(format!("spi byte {x} out of range 0..255"));
                    let data = match t.get("data") {
                        Some(d) => d.as_array().ok_or("spi.data must be an array")?.iter().map(byte).collect::<Result<_, _>>()?,
                        None => Default::default(),
                    };
                    let fill = t.get("fill").map_or(Ok(0), byte)?;
                    let port = t.get("port").map_or(Ok(1), |x| x.as_integer().and_then(|i| u8::try_from(i).ok()).ok_or("bad spi.port"))?;
                    st.spi.push((port, SpiClient { data, fill }));
                }
                continue;
            }
            if k == "i2c" {
                for e in v.as_array().ok_or("i2c must be [[i2c]] entries")? {
                    let t = e.as_table().ok_or("i2c entry must be a table")?;
                    for k in t.keys() {
                        if !["port", "address", "data", "fill"].contains(&k.as_str()) {
                            return Err(format!("i2c: unknown key {k}"));
                        }
                    }
                    let byte = |x: &toml::Value| x.as_integer().and_then(|i| u8::try_from(i).ok()).ok_or(format!("i2c byte {x} out of range 0..255"));
                    let address = match t.get("address").map(byte) {
                        Some(Ok(a)) if a < 128 => a,
                        Some(Ok(a)) => return Err(format!("i2c.address {a} is not a 7-bit address")),
                        Some(Err(e)) => return Err(e),
                        None => return Err("i2c needs an address".into()),
                    };
                    let data = match t.get("data") {
                        Some(d) => d.as_array().ok_or("i2c.data must be an array")?.iter().map(byte).collect::<Result<_, _>>()?,
                        None => Default::default(),
                    };
                    let fill = t.get("fill").map_or(Ok(0xFF), byte)?;
                    let port = t.get("port").map_or(Ok(1), |x| x.as_integer().and_then(|i| u8::try_from(i).ok()).ok_or("bad i2c.port"))?;
                    if st.i2c.iter().any(|(p, c)| *p == port && c.address == address) {
                        return Err(format!("i2c: address {address:#x} given twice on port {port}"));
                    }
                    st.i2c.push((port, I2cClient { address, data, fill, ..Default::default() }));
                }
                continue;
            }
            if k == "adc" {
                for e in v.as_array().ok_or("adc must be [[adc]] entries")? {
                    let t = e.as_table().ok_or("adc entry must be a table")?;
                    for k in t.keys() {
                        if !["channel", "value", "at"].contains(&k.as_str()) {
                            return Err(format!("adc: unknown key {k}"));
                        }
                    }
                    let int = |n: &str, max: u64| -> Result<Option<u64>, String> {
                        t.get(n).map_or(Ok(None), |x| {
                            x.as_integer().and_then(|i| u64::try_from(i).ok()).filter(|i| *i <= max).map(Some).ok_or(format!("adc.{n} must be an integer 0..{max}"))
                        })
                    };
                    st.adc.push((
                        int("channel", 63)?.ok_or("adc needs channel")? as u8,
                        int("at", MAX_CYCLES)?.unwrap_or(0),
                        int("value", 0xFFFF)?.ok_or("adc needs value")? as u16,
                    ));
                }
                continue;
            }
            if k != "uart" {
                return Err(format!("unknown section [{k}]"));
            }
            for e in v.as_array().ok_or("uart must be [[uart]] entries")? {
                let t = e.as_table().ok_or("uart entry must be a table")?;
                let int = |n: &str| -> Result<Option<u64>, String> {
                    match t.get(n) {
                        None => Ok(None),
                        Some(x) => {
                            let v = x.as_integer().and_then(|i| u64::try_from(i).ok()).ok_or(format!("uart.{n} must be a non-negative integer"))?;
                            if n != "port" && v > MAX_CYCLES {
                                return Err(format!("uart.{n} must be at most {MAX_CYCLES}"));
                            }
                            Ok(Some(v))
                        }
                    }
                };
                let flag = |n: &str| t.get(n).map_or(Ok(false), |x| x.as_bool().ok_or(format!("uart.{n} must be true or false")));
                for k in t.keys() {
                    if !["port", "at", "bytes", "data", "bit", "gap", "nine", "bad_stop"].contains(&k.as_str()) {
                        return Err(format!("uart: unknown key {k}"));
                    }
                }
                let nine = flag("nine")?;
                let words: Vec<u16> = match (t.get("bytes"), t.get("data")) {
                    (Some(b), None) => b.as_str().ok_or("uart.bytes must be a string")?.chars().map(|c| c as u16 & 0xFF).collect(),
                    (None, Some(d)) => d
                        .as_array()
                        .ok_or("uart.data must be an array")?
                        .iter()
                        .map(|x| {
                            x.as_integer()
                                .and_then(|i| u16::try_from(i).ok())
                                .filter(|i| *i <= if nine { 0x1FF } else { 0xFF })
                                .ok_or(format!("uart.data value {x} out of range"))
                        })
                        .collect::<Result<_, _>>()?,
                    _ => return Err("uart needs exactly one of bytes or data".into()),
                };
                let bit = int("bit")?;
                if bit == Some(0) {
                    return Err("uart.bit must be at least 1".into());
                }
                let port = int("port")?.unwrap_or(1);
                st.uart.push((
                    u8::try_from(port).map_err(|_| "bad uart.port")?,
                    RxStream { at: int("at")?.unwrap_or(0), words: words.into(), bit, gap: int("gap")?.unwrap_or(0), nine, bad_stop: flag("bad_stop")? },
                ));
            }
        }
        Ok(st)
    }

    /// Hand the characters to the receivers of `cpu`. An EUSART the part does not have is an error.
    pub fn apply(self, dev: &Device, p: &mut crate::periph::Periph) -> Result<(), String> {
        for (port, s) in self.uart {
            if !p.rx_send(port, s) {
                return Err(format!("{}: no EUSART{port} receiver in the simulator", dev.device.name));
            }
        }
        for (port, c) in self.spi {
            if !p.spi_client(port, c) {
                return Err(format!("{}: no SPI{port} host in the simulator", dev.device.name));
            }
        }
        for (port, c) in self.i2c {
            if !p.i2c_client(port, c) {
                return Err(format!("{}: no MSSP{port} in the simulator", dev.device.name));
            }
        }
        if !self.board.is_empty() {
            let letters: Vec<u8> = self.board.leds.iter().map(|l| l.pin.0).chain(self.board.oleds.iter().flat_map(|o| [o.scl.0, o.sda.0])).collect();
            for l in letters {
                if !p.has_port(l) {
                    return Err(format!("{}: no PORT{} in the simulator", dev.device.name, l as char));
                }
            }
            p.set_board(self.board);
        }
        for (letter, at, mask, val) in self.pins {
            if !p.pin_feed(letter, at, mask, val) {
                return Err(format!("{}: no PORT{} in the simulator", dev.device.name, letter as char));
            }
        }
        if !self.adc.is_empty() && !self.adc.iter().all(|(c, at, v)| p.adc_feed(*c, *at, *v)) {
            return Err(format!("{}: no ADC model in the simulator", dev.device.name));
        }
        Ok(())
    }
}
