//! Stimulus file: what the outside world does to the part over time (TOML).
//! Time fields accept cycle integers or strings with ns/us/ms/s/cycles; physical units require explicit `fosc_mhz`.
//! `[[clock]]` supplies periodic pin edges, `[[uart_device]]` repeatable request/reply rules,
//! and `[[i2c]] mode = "memory"` a device with 256 registers and a wrapping pointer (see docs/MANUAL.md).
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
//! [[analog]]         # the voltage on a pin, for the comparators
//! pin = "RA0"
//! volts = 1.5         # 0 .. 6
//! at = 0             # from this instruction cycle on; later entries replace earlier ones
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
//! [clb]              # the CLB netlist of a PIC16F131xx / PIC16F132xx (see docs/MANUAL.md "CLB netlist")
//! divider = 8         # BLE_clk = CLBCLK / divider
//! [[clb.ble]]         # one of the 32 BLEs: n, a, b, c, d (input selections), lut (16 bits), flop
//! [[clb.input]]       # one of the 16 input selections: n, source, edge ("none", "pos", "neg"), sync
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
//! [[bounce]]          # a contact that chatters before it settles: 1 + 2 x bounces changes of one input pin
//! pin = "RB0"         # the PIC pin
//! level = 0           # 0 or 1 (also true/false): the level the pin goes to at `at` and settles at
//! at = 20000          # the first change, default 0
//! bounces = 3         # times the contact springs back before it settles, 0..1000
//! period = 150        # cycles between changes (1..2^32), or a range [100, 400] that each interval is drawn from
//! seed = 1            # with a range: the same seed gives the same intervals on every run and after a reset (default 1)
//!
//! [[mclr]]            # the level of the MCLR pin (RE3 on the 28-pin Q10): 0 holds the part in reset (when the pin is MCLR)
//! level = 0           # 0 or 1; from `at` on, later entries replace earlier ones; the pin is high until driven
//! at = 20000
//!
//! fosc_mhz = 64       # optional: the PIC clock, for the OLED's scrolling time and the watchdog (default follows firmware oscillator settings)
//!
//! [[led]]             # an LED on a pin: on while the pin is an output at the active level
//! pin = "RB0"         # the PIC pin the LED is wired to
//! active = "high"     # "high" (default) or "low" (the LED is wired to Vdd through the pin)
//! name = "LED1"       # optional
//!
//! [[oled]]            # an SSD1306 OLED (I2C); on a PIC16 part the hardware bus is the MSSP in I2C host mode
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
    /// Periodic input pins, evaluated lazily by the pin driver.
    pub clocks: Vec<crate::drive::Clock>,
    /// (UART number, repeatable request/reply rule).
    pub uart_devices: Vec<(u8, crate::periph::UartReply)>,
    /// (SPI number, client)
    pub spi: Vec<(u8, SpiClient)>,
    /// (MSSP number, client)
    pub i2c: Vec<(u8, I2cClient)>,
    /// (channel, from cycle, code)
    pub adc: Vec<(u8, u64, u16)>,
    /// (port letter, pin, from cycle, volts)
    pub analog: Vec<(u8, u8, u64, f64)>,
    /// (port letter, from cycle, pin mask, levels): the [[bounce]] changes first, then [[pin]] and [[port]].
    pub pins: Vec<(u8, u64, u8, u8)>,
    /// The pins [[bounce]] entries name, checked against the part when the stimulus is applied.
    pub bounce_pins: Vec<crate::periph::Pin>,
    /// (from cycle, level) of the MCLR pin, sorted by cycle.
    pub mclr: Vec<(u64, u8)>,
    /// LEDs and OLEDs on the board.
    pub board: crate::periph::Board,
    /// An explicit clock key, including zero to restore firmware timing.
    pub fosc_given: bool,
    /// The CLB netlist (`[clb]`, PIC16F131xx and PIC16F132xx).
    pub clb: Option<crate::periph::clb::Cfg>,
}

impl Stimulus {
    pub fn parse(text: &str) -> Result<Stimulus, String> {
        let mut doc: toml::Table = text.parse().map_err(|e: toml::de::Error| e.to_string())?;
        normalize_times(&mut doc)?;
        let mut st = Stimulus::default();
        // [[bounce]] changes are kept apart and put before the [[pin]]/[[port]] ones at the end, whatever order the keys
        // come in: at a cycle both name, the [[pin]]/[[port]] level wins (`Drive::push` keeps the order changes were added in).
        let mut bounce = Vec::new();
        let mut trains: Vec<(crate::periph::Pin, u64, u64)> = Vec::new();
        for (k, v) in &doc {
            if k == "clock" {
                for e in v.as_array().ok_or("clock must be [[clock]] entries")? {
                    let t = e.as_table().ok_or("clock entry must be a table")?;
                    for key in t.keys() {
                        if !["pin", "at", "period", "high", "until"].contains(&key.as_str()) { return Err(format!("clock: unknown key {key}")); }
                    }
                    let pin = t.get("pin").and_then(|v| v.as_str()).and_then(parse_pin).ok_or("clock needs pin (RA0..RH7)")?;
                    if st.clocks.iter().any(|c| c.pin == pin) { return Err("clock: only one clock per pin is supported".into()); }
                    let time = |key: &str| -> Result<Option<u64>, String> {
                        t.get(key).map(|v| v.as_integer().and_then(|v| u64::try_from(v).ok()).filter(|v| *v <= MAX_CYCLES).ok_or(format!("clock.{key} must be 0..{MAX_CYCLES}"))).transpose()
                    };
                    let at = time("at")?.unwrap_or(0);
                    let period = time("period")?.ok_or("clock needs period")?;
                    let high = time("high")?.unwrap_or(period / 2);
                    let until = time("until")?;
                    if high == 0 || high >= period { return Err("clock.high must be at least 1 and less than period".into()); }
                    if until.is_some_and(|end| end <= at) { return Err("clock.until must be after at".into()); }
                    st.clocks.push(crate::drive::Clock { pin, at, period, high, until });
                }
                continue;
            }
            if k == "uart_device" {
                for e in v.as_array().ok_or("uart_device must be [[uart_device]] entries")? {
                    let t = e.as_table().ok_or("uart_device entry must be a table")?;
                    for key in t.keys() {
                        if !["port", "request", "reply", "delay", "bit", "gap"].contains(&key.as_str()) { return Err(format!("uart_device: unknown key {key}")); }
                    }
                    let bytes = |key: &str| -> Result<Vec<u8>, String> {
                        match t.get(key) {
                            Some(toml::Value::String(s)) => s.chars().map(|c| u8::try_from(c).map_err(|_| format!("uart_device.{key}: character is not one byte"))).collect(),
                            Some(toml::Value::Array(a)) => a.iter().map(|v| v.as_integer().and_then(|v| u8::try_from(v).ok()).ok_or(format!("uart_device.{key}: expected bytes 0..255"))).collect(),
                            _ => Err(format!("uart_device needs {key} (string or byte array)")),
                        }
                    };
                    let time = |key: &str| -> Result<Option<u64>, String> { t.get(key).map(|v| v.as_integer().and_then(|v| u64::try_from(v).ok()).filter(|v| *v <= MAX_CYCLES).ok_or(format!("uart_device.{key}: expected 0..{MAX_CYCLES}"))).transpose() };
                    let port = t.get("port").map_or(Ok(1), |v| v.as_integer().and_then(|v| u8::try_from(v).ok()).filter(|v| *v > 0).ok_or("bad uart_device.port"))?;
                    let request = bytes("request")?;
                    if request.is_empty() { return Err("uart_device.request must not be empty".into()); }
                    let bit = time("bit")?;
                    if bit == Some(0) { return Err("uart_device.bit must be positive".into()); }
                    st.uart_devices.push((port, crate::periph::UartReply::new(request, bytes("reply")?, time("delay")?.unwrap_or(0), bit, time("gap")?.unwrap_or(0))));
                }
                continue;
            }
            if k == "bounce" {
                for e in v.as_array().ok_or("bounce must be [[bounce]] entries")? {
                    let t = e.as_table().ok_or("bounce entry must be a table")?;
                    let (pin, train) = parse_bounce(t)?;
                    let (from, to) = (train[0].1, train[train.len() - 1].1);
                    if let Some(&(_, f, l)) = trains.iter().find(|&&(p, f, l)| p == pin && from <= l && f <= to) {
                        let name = crate::periph::pin_name(pin);
                        return Err(format!("bounce on {name}: cycles {from}..{to} overlap another bounce on {name} ({f}..{l}); start the next one after the last change"));
                    }
                    trains.push((pin, from, to));
                    if !st.bounce_pins.contains(&pin) {
                        st.bounce_pins.push(pin);
                    }
                    bounce.extend(train);
                }
                continue;
            }
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
            if k == "mclr" {
                for e in v.as_array().ok_or("mclr must be [[mclr]] entries")? {
                    let t = e.as_table().ok_or("mclr entry must be a table")?;
                    for key in t.keys() {
                        if !["level", "at"].contains(&key.as_str()) {
                            return Err(format!("mclr: unknown key {key}"));
                        }
                    }
                    let level = match t.get("level") {
                        Some(toml::Value::Boolean(b)) => u8::from(*b),
                        Some(x) if matches!(x.as_integer(), Some(0 | 1)) => x.as_integer().unwrap() as u8,
                        _ => return Err("mclr.level must be 0, 1, true or false".into()),
                    };
                    let at = match t.get("at") {
                        None => 0,
                        Some(x) => x.as_integer().and_then(|i| u64::try_from(i).ok()).filter(|i| *i <= MAX_CYCLES).ok_or(format!("mclr.at must be an integer 0..{MAX_CYCLES}"))?,
                    };
                    st.mclr.push((at, level));
                }
                st.mclr.sort_by_key(|e| e.0);
                continue;
            }
            if k == "clb" {
                st.clb = Some(parse_clb(v.as_table().ok_or("clb must be a [clb] table")?)?);
                continue;
            }
            if k == "fosc_mhz" {
                st.fosc_given = true;
                let f = v.as_float().or_else(|| v.as_integer().map(|i| i as f64)).filter(|f| *f >= 0.0 && *f <= 64.0);
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
                        if !["port", "address", "data", "fill", "mode", "readonly"].contains(&k.as_str()) {
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
                    let data: std::collections::VecDeque<u8> = match t.get("data") {
                        Some(d) => d.as_array().ok_or("i2c.data must be an array")?.iter().map(byte).collect::<Result<_, _>>()?,
                        None => Default::default(),
                    };
                    let fill = t.get("fill").map_or(Ok(0xFF), byte)?;
                    let port = t.get("port").map_or(Ok(1), |x| x.as_integer().and_then(|i| u8::try_from(i).ok()).ok_or("bad i2c.port"))?;
                    if st.i2c.iter().any(|(p, c)| *p == port && c.address == address) {
                        return Err(format!("i2c: address {address:#x} given twice on port {port}"));
                    }
                    let readonly = t.get("readonly").map_or(Ok(false), |v| v.as_bool().ok_or("i2c.readonly must be boolean"))?;
                    let memory = match t.get("mode").map(|v| v.as_str()) {
                        None | Some(Some("stream")) => {
                            if t.contains_key("readonly") { return Err("i2c.readonly requires mode = \"memory\"".into()); }
                            None
                        }
                        Some(Some("memory")) => {
                            if data.len() > 256 { return Err("i2c memory data must contain at most 256 bytes".into()); }
                            let mut memory = Box::new([fill; 256]);
                            for (i, b) in data.iter().enumerate() { memory[i] = *b; }
                            Some(memory)
                        }
                        _ => return Err("i2c.mode must be \"stream\" or \"memory\"".into()),
                    };
                    st.i2c.push((port, I2cClient { address, data, fill, memory, readonly, ..Default::default() }));
                }
                continue;
            }
            if k == "analog" {
                for e in v.as_array().ok_or("analog must be [[analog]] entries")? {
                    let t = e.as_table().ok_or("analog entry must be a table")?;
                    for key in t.keys() {
                        if !["pin", "volts", "at"].contains(&key.as_str()) {
                            return Err(format!("analog: unknown key {key}"));
                        }
                    }
                    let s = t.get("pin").and_then(|x| x.as_str()).ok_or("analog needs pin (a pin such as \"RA0\")")?;
                    let (letter, bit) = parse_pin(s).ok_or(format!("analog.pin = {s:?} is not a pin (RA0..RH7)"))?;
                    let volts = match t.get("volts") {
                        Some(toml::Value::Float(f)) => *f,
                        Some(toml::Value::Integer(i)) => *i as f64,
                        _ => return Err("analog needs volts (a number)".into()),
                    };
                    if !(0.0..=6.0).contains(&volts) {
                        return Err(format!("analog.volts {volts} must be 0..6"));
                    }
                    let at = match t.get("at") {
                        None => 0,
                        Some(x) => x.as_integer().and_then(|i| u64::try_from(i).ok()).filter(|i| *i <= MAX_CYCLES).ok_or(format!("analog.at must be an integer 0..{MAX_CYCLES}"))?,
                    };
                    st.analog.push((letter, bit, at, volts));
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
                    (Some(b), None) => b.as_str().ok_or("uart.bytes must be a string")?.chars().map(|c| u8::try_from(c).map(u16::from).map_err(|_| format!("uart.bytes: {c:?} is not one byte (use uart.data for byte values)"))).collect::<Result<_, _>>()?,
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
        st.pins.splice(0..0, bounce);
        Ok(st)
    }

    /// The same for a PIC16 core, which models the EUSART receivers, the ADC inputs and the port pins only: any other section is an
    /// error rather than being ignored.
    pub fn apply16(self, dev: &Device, cpu: &mut crate::pic16::Pic16) -> Result<(), String> {
        let other = [
            ("spi", !self.spi.is_empty()),
            ("i2c", !self.i2c.is_empty()),
            ("analog", !self.analog.is_empty()),
            ("mclr", !self.mclr.is_empty()),
        ];
        if self.clb.is_some() && !cpu.has_clb() {
            return Err(format!("{}: the part has no CLB", dev.device.name));
        }
        if let Some((n, _)) = other.iter().find(|(_, used)| *used) {
            return Err(format!("{}: [[{n}]] is not modelled on PIC16 parts yet; only [[uart]], [[uart_device]], [[adc]], [[pin]]/[[port]], [[bounce]], [[clock]], [[led]] and [[oled]] are", dev.device.name));
        }
        for o in &self.board.oleds {
            if let Bus::I2c(n) = o.bus
                && !cpu.has_mssp(n)
            {
                return Err(format!("{}: the part has no MSSP{n} in the simulator; an [[oled]] needs bus = \"gpio\" (bit-banged) or an MSSP the part has", dev.device.name));
            }
        }
        if let Some((port, _)) = self.uart_devices.iter().find(|(port, _)| !cpu.has_uart(*port)) { return Err(format!("{}: no UART{port}", dev.device.name)); }
        let pins = cpu.pins();
        for p in self.board.leds.iter().map(|l| l.pin).chain(self.board.oleds.iter().flat_map(|o| [o.scl, o.sda])).chain(self.bounce_pins.iter().copied()).chain(self.clocks.iter().map(|c| c.pin)) {
            if !pins.contains(&p) {
                return Err(format!("{}: no pin {} on this part", dev.device.name, crate::periph::pin_name(p)));
            }
        }
        if !self.adc.is_empty() && !cpu.has_adc() {
            return Err(format!("{}: no ADC model in the simulator", dev.device.name));
        }
        if let Some((port, _)) = self.uart.iter().find(|(p, _)| !cpu.has_uart(*p)) {
            return Err(format!("{}: no EUSART{port} receiver in the simulator", dev.device.name)); // nothing is queued
        }
        if let Some((l, ..)) = self.pins.iter().find(|p| !cpu.has_port(p.0)) {
            return Err(format!("{}: no PORT{} in the simulator", dev.device.name, *l as char));
        }
        if self.board.fosc_mhz > dev.fosc_max_mhz() {
            return Err(format!("{}: fosc_mhz = {} is above the part's {} MHz", dev.device.name, self.board.fosc_mhz, dev.fosc_max_mhz()));
        }
        let clock = self.fosc_given.then_some(self.board.fosc_mhz);
        if !self.board.is_empty() {
            cpu.set_board(self.board);
        } else if self.board.fosc_mhz > 0.0 {
            cpu.board.fosc_mhz = self.board.fosc_mhz; // a clock without parts still sets the timing
        }
        if let Some(f) = clock { cpu.board.fosc_mhz = f; }
        if let Some(c) = self.clb {
            cpu.clb_set(c);
        }
        for (letter, at, mask, val) in self.pins {
            cpu.pin_feed(letter, at, mask, val);
        }
        for (ch, at, v) in self.adc {
            cpu.adc_feed(ch, at, v);
        }
        for clock in self.clocks { cpu.clock_feed(clock); }
        for (port, rule) in self.uart_devices { cpu.uart_reply(port, rule); }
        for (port, s) in self.uart {
            cpu.rx_send(port, s);
        }
        Ok(())
    }

    /// Hand the characters to the receivers of `cpu`. An EUSART the part does not have is an error.
    pub fn apply(self, dev: &Device, m: &crate::mem::DataMem, p: &mut crate::periph::Periph) -> Result<(), String> {
        if self.clb.is_some() {
            return Err(format!("{}: the part has no CLB", dev.device.name));
        }
        for clock in self.clocks {
            if !p.has_pin(m, clock.pin) { return Err(format!("{}: no pin {}", dev.device.name, crate::periph::pin_name(clock.pin))); }
            p.clock_feed(clock);
        }
        for (port, rule) in self.uart_devices {
            if !p.uart_reply(port, rule) { return Err(format!("{}: no UART{port}", dev.device.name)); }
        }
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
        let clock = self.fosc_given.then_some(self.board.fosc_mhz);
        if !self.board.is_empty() {
            // the pin itself, not only its port letter: a bit the port does not implement would never light (as `apply16`)
            for pin in self.board.leds.iter().map(|l| l.pin).chain(self.board.oleds.iter().flat_map(|o| [o.scl, o.sda])) {
                if !p.has_pin(m, pin) {
                    return Err(format!("{}: no pin {} on this part", dev.device.name, crate::periph::pin_name(pin)));
                }
            }
            // an I2C host the part lacks would leave the OLED "NOT WIRED" only at run time (as `apply16` refuses it)
            if let Some(n) = self.board.oleds.iter().filter_map(|o| if let Bus::I2c(n) = o.bus { Some(n) } else { None }).find(|&n| !p.has_i2c_host(n)) {
                return Err(format!("{}: the part has no I2C host {n} (MSSP{n} or I2C{n}) in the simulator; an [[oled]] needs bus = \"gpio\" (bit-banged) or a host the part has", dev.device.name));
            }
            p.set_board(self.board);
        } else if self.board.fosc_mhz > 0.0 {
            p.board.fosc_mhz = self.board.fosc_mhz; // a clock without parts still sets the timing
        }
        if let Some(f) = clock { p.board.fosc_mhz = f; }
        if let Some(&pin) = self.bounce_pins.iter().find(|&&pin| !p.has_pin(m, pin)) {
            return Err(format!("{}: [[bounce]] on {}: no such pin on this part", dev.device.name, crate::periph::pin_name(pin)));
        }
        for (letter, at, mask, val) in self.pins {
            if !p.pin_feed(letter, at, mask, val) {
                return Err(format!("{}: no PORT{} in the simulator", dev.device.name, letter as char));
            }
        }
        for (letter, bit, at, volts) in self.analog {
            if !p.analog_feed(letter, bit, at, volts) {
                return Err(format!("{}: no PORT{} in the simulator", dev.device.name, letter as char));
            }
        }
        if !self.adc.is_empty() && !self.adc.iter().all(|(c, at, v)| p.adc_feed(*c, *at, *v)) {
            return Err(format!("{}: no ADC model in the simulator", dev.device.name));
        }
        Ok(())
    }
}

/// Resolve physical times once at load time; firmware oscillator changes do not retime a file.
fn normalize_times(doc: &mut toml::Table) -> Result<(), String> {
    let fosc = doc.get("fosc_mhz").and_then(|v| v.as_float().or_else(|| v.as_integer().map(|i| i as f64)));
    for section in ["uart", "uart_device", "pin", "port", "bounce", "clock", "mclr", "adc", "analog"] {
        let Some(entries) = doc.get_mut(section).and_then(|v| v.as_array_mut()) else { continue; };
        for entry in entries {
            let Some(t) = entry.as_table_mut() else { continue; };
            let keys: &[&str] = match section {
                "uart" => &["at", "bit", "gap"],
                "uart_device" => &["delay", "bit", "gap"],
                "bounce" => &["at", "period"],
                "clock" => &["at", "period", "high", "until"],
                _ => &["at"],
            };
            for key in keys {
                if let Some(value) = t.get_mut(*key) {
                    if let Some(a) = value.as_array_mut() {
                        for v in a { normalize_time(v, fosc, section, key)?; }
                    } else { normalize_time(value, fosc, section, key)?; }
                }
            }
        }
    }
    Ok(())
}

fn normalize_time(value: &mut toml::Value, fosc: Option<f64>, section: &str, key: &str) -> Result<(), String> {
    let Some(text) = value.as_str() else { return Ok(()); };
    let error = || format!("{section}.{key}: expected a non-negative time with ns, us, ms, s or cycles (maximum {MAX_CYCLES} cycles)");
    let text = text.trim();
    let (number, scale) = if let Some(n) = text.strip_suffix("cycles") { (n, 1.0) }
        else {
            let (n, seconds) = [("ns", 1e-9), ("us", 1e-6), ("ms", 1e-3), ("s", 1.0)].into_iter()
                .find_map(|(unit, scale)| text.strip_suffix(unit).map(|n| (n, scale))).ok_or_else(error)?;
            let mhz = fosc.filter(|f| f.is_finite() && *f > 0.0 && *f <= 64.0)
                .ok_or(format!("{section}.{key}: physical time units require an explicit positive fosc_mhz"))?;
            (n, seconds * mhz * 1e6 / 4.0)
        };
    let number: f64 = number.trim().parse().map_err(|_| error())?;
    let cycles = number * scale;
    if !number.is_finite() || number < 0.0 || !cycles.is_finite() || cycles > MAX_CYCLES as f64 { return Err(error()); }
    // Ignore floating point noise at integer boundaries, then round towards the next instruction cycle.
    let rounded = cycles.round();
    let cycles = if (cycles - rounded).abs() <= 4.0 * f64::EPSILON * cycles.abs() { rounded } else { cycles.ceil() };
    *value = toml::Value::Integer(cycles as i64);
    Ok(())
}

/// A stimulus pin change: (port letter, from cycle, pin mask, levels).
type PinChange = (u8, u64, u8, u8);

/// Largest `bounces` of a `[[bounce]]` entry.
const MAX_BOUNCES: u64 = 1000;

/// One `[[bounce]]` entry: its pin and its pin changes `(port letter, at, mask, levels)`: `level` at `at`, then
/// `bounces` times the other level and back, one interval apart. The intervals are `period`, or drawn from the range
/// `[min, max]` by SplitMix64 from `seed`: a function of the entry alone, with no other state, so a reset that parses the
/// file again (or keeps what it parsed) gets the same train.
fn parse_bounce(t: &toml::Table) -> Result<(crate::periph::Pin, Vec<PinChange>), String> {
    for key in t.keys() {
        if !["pin", "level", "at", "bounces", "period", "seed"].contains(&key.as_str()) {
            return Err(format!("bounce: unknown key {key}"));
        }
    }
    let s = t.get("pin").and_then(|x| x.as_str()).ok_or("bounce needs pin (a pin such as \"RB0\")")?;
    let (letter, bit) = parse_pin(s).ok_or(format!("bounce.pin = {s:?} is not a pin (RA0..RH7)"))?;
    let level = match t.get("level") {
        Some(toml::Value::Boolean(b)) => u8::from(*b),
        Some(x) if matches!(x.as_integer(), Some(0 | 1)) => x.as_integer().unwrap() as u8,
        _ => return Err("bounce.level must be 0, 1, true or false".into()),
    };
    let int = |x: &toml::Value, n: &str, min: u64, max: u64| -> Result<u64, String> {
        x.as_integer().and_then(|i| u64::try_from(i).ok()).filter(|i| (min..=max).contains(i)).ok_or(format!("bounce.{n} must be an integer {min}..{max}"))
    };
    let at = t.get("at").map_or(Ok(0), |x| int(x, "at", 0, MAX_CYCLES))?;
    let bounces = int(t.get("bounces").ok_or("bounce needs bounces")?, "bounces", 0, MAX_BOUNCES)?;
    let period = t.get("period").ok_or("bounce needs period")?;
    let (lo, hi) = match period {
        toml::Value::Array(a) if a.len() == 2 => {
            let (lo, hi) = (int(&a[0], "period", 1, MAX_CYCLES)?, int(&a[1], "period", 1, MAX_CYCLES)?);
            if lo > hi {
                return Err(format!("bounce.period [{lo}, {hi}]: the first value must not be above the second"));
            }
            (lo, hi)
        }
        toml::Value::Array(_) => return Err("bounce.period range must be [min, max]".into()),
        x => {
            let p = int(x, "period", 1, MAX_CYCLES)?;
            (p, p)
        }
    };
    let mut seed = match t.get("seed") {
        Some(_) if !period.is_array() => return Err("bounce.seed needs a period range [min, max]".into()),
        Some(x) => x.as_integer().and_then(|i| u64::try_from(i).ok()).ok_or("bounce.seed must be a non-negative integer")?,
        None => 1,
    };
    let mut next = || {
        // SplitMix64 (Steele, Lea, Flood 2014); the modulo bias is below 2^-31 for spans up to 2^32
        seed = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = seed;
        z = (z ^ z >> 30).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ z >> 27).wrapping_mul(0x94D0_49BB_1331_11EB);
        lo + (z ^ z >> 31) % (hi - lo + 1)
    };
    let mask = 1u8 << bit;
    let mut train = vec![(letter, at, mask, level << bit)];
    let mut c = at;
    for i in 0..2 * bounces {
        c += next(); // at most 2^32 + 2000 x 2^32: no overflow
        if c > MAX_CYCLES {
            return Err(format!("bounce on {s}: the last change would be after cycle {MAX_CYCLES}"));
        }
        let l = if i % 2 == 0 { level ^ 1 } else { level };
        train.push((letter, c, mask, l << bit));
    }
    Ok(((letter, bit), train))
}

/// The `[clb]` netlist: the latches of DS40002686B Fig. 29-1 to 29-13, which the CLB Synthesizer would load from program memory.
fn parse_clb(t: &toml::Table) -> Result<crate::periph::clb::Cfg, String> {
    use crate::periph::clb::{BleCfg, Cfg, CounterCfg, Edge};
    let num = |v: &toml::Value, what: &str, max: i64| -> Result<i64, String> {
        v.as_integer().filter(|i| (0..=max).contains(i)).ok_or(format!("clb.{what} must be an integer 0..{max}"))
    };
    let known = |t: &toml::Table, keys: &[&str], what: &str| -> Result<(), String> {
        t.keys().find(|k| !keys.contains(&k.as_str())).map_or(Ok(()), |k| Err(format!("{what}: unknown key {k}")))
    };
    known(t, &["divider", "output", "irq", "counter", "input", "ble"], "clb")?;
    let mut c = Cfg { given: true, ..Cfg::default() };
    if let Some(v) = t.get("divider") {
        c.divider = u32::try_from(num(v, "divider", 1 << 16)?).map_err(|_| "clb.divider")?.max(1);
    }
    if let Some(v) = t.get("output") {
        let a = v.as_array().filter(|a| a.len() <= 8).ok_or("clb.output must be an array of up to 8 selections 0..3")?;
        for (n, x) in a.iter().enumerate() {
            c.outsel[n] = num(x, "output", 3)? as u8;
        }
    }
    if let Some(v) = t.get("irq") {
        let a = v.as_array().filter(|a| a.len() <= 4).ok_or("clb.irq must be an array of up to 4 BLE numbers (-1 for none)")?;
        for (n, x) in a.iter().enumerate() {
            c.irq[n] = match x.as_integer() {
                Some(-1) => None,
                _ => Some(num(x, "irq", 31)? as u8),
            };
        }
    }
    if let Some(v) = t.get("counter") {
        let k = v.as_table().ok_or("clb.counter must be a table")?;
        known(k, &["enable", "stop", "reset", "a1", "a2", "b1", "b2", "c1", "c2", "d1", "d2"], "clb.counter")?;
        let mut n = CounterCfg { enable: k.get("enable").map_or(Ok(true), |x| x.as_bool().ok_or("clb.counter.enable must be true or false"))?, ..CounterCfg::default() };
        n.stop = k.get("stop").map(|x| num(x, "counter.stop", 31).map(|i| i as u8)).transpose()?;
        n.reset = k.get("reset").map(|x| num(x, "counter.reset", 31).map(|i| i as u8)).transpose()?;
        for (i, name) in ["a1", "a2", "b1", "b2", "c1", "c2", "d1", "d2"].iter().enumerate() {
            n.sel[i] = k.get(*name).map(|x| num(x, "counter.select", 7).map(|i| i as u8)).transpose()?;
        }
        c.counter = n;
    }
    for e in t.get("input").map_or(Ok(&[][..]), |v| v.as_array().map(Vec::as_slice).ok_or("clb.input must be [[clb.input]] entries"))? {
        let k = e.as_table().ok_or("clb.input entry must be a table")?;
        known(k, &["n", "source", "edge", "sync"], "clb.input")?;
        let n = num(k.get("n").ok_or("clb.input needs n")?, "input.n", 15)? as usize;
        let i = &mut c.inputs[n];
        i.source = num(k.get("source").ok_or("clb.input needs source (the Table 29-1 value)")?, "input.source", 63)? as u8;
        i.edge = match k.get("edge").map(|x| x.as_str()) {
            None | Some(Some("none")) => Edge::None,
            Some(Some("pos")) => Edge::Pos,
            Some(Some("neg")) => Edge::Neg,
            _ => return Err("clb.input.edge must be \"none\", \"pos\" or \"neg\"".into()),
        };
        i.sync = k.get("sync").map_or(Ok(false), |x| x.as_bool().ok_or("clb.input.sync must be true or false"))?;
    }
    for e in t.get("ble").map_or(Ok(&[][..]), |v| v.as_array().map(Vec::as_slice).ok_or("clb.ble must be [[clb.ble]] entries"))? {
        let k = e.as_table().ok_or("clb.ble entry must be a table")?;
        known(k, &["n", "a", "b", "c", "d", "lut", "flop"], "clb.ble")?;
        let n = num(k.get("n").ok_or("clb.ble needs n")?, "ble.n", 31)? as usize;
        let mut b = BleCfg { sel: [31; 4], lut: 0, flop: false };
        for (x, name) in ["a", "b", "c", "d"].iter().enumerate() {
            if let Some(v) = k.get(*name) {
                b.sel[x] = num(v, "ble.input", 31)? as u8;
            }
        }
        b.lut = num(k.get("lut").ok_or("clb.ble needs lut (16 bits)")?, "ble.lut", 0xFFFF)? as u16;
        b.flop = k.get("flop").map_or(Ok(false), |x| x.as_bool().ok_or("clb.ble.flop must be true or false"))?;
        c.ble[n] = b;
    }
    Ok(c)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn changes(text: &str) -> Vec<(u64, u8)> {
        Stimulus::parse(text).unwrap().pins.iter().map(|&(l, at, m, v)| {
            assert_eq!((l, m), (b'B', 1), "RB0");
            (at, v)
        }).collect()
    }

    #[test]
    fn bounce_with_a_fixed_period() {
        let c = changes("[[bounce]]\npin = \"RB0\"\nlevel = 1\nat = 100\nbounces = 2\nperiod = 10\n");
        assert_eq!(c, [(100, 1), (110, 0), (120, 1), (130, 0), (140, 1)]);
        let c = changes("[[bounce]]\npin = \"RB0\"\nlevel = false\nbounces = 0\nperiod = 10\n");
        assert_eq!(c, [(0, 0)], "no bounces: one clean change at cycle 0");
    }

    #[test]
    fn bounce_with_a_range_is_repeatable_and_inside_it() {
        let text = "[[bounce]]\npin = \"RB0\"\nlevel = 0\nat = 5\nbounces = 50\nperiod = [3, 9]\nseed = 7\n";
        let c = changes(text);
        assert_eq!(c, changes(text), "the same file gives the same train");
        assert_eq!(c.len(), 101);
        assert_eq!(*c.last().unwrap(), (c.last().unwrap().0, 0), "settles at level");
        let gaps: Vec<u64> = c.windows(2).map(|w| w[1].0 - w[0].0).collect();
        assert!(gaps.iter().all(|g| (3..=9).contains(g)), "{gaps:?}");
        assert!(gaps.iter().any(|g| *g != gaps[0]), "the intervals vary");
        assert_ne!(c, changes(&text.replace("seed = 7", "seed = 8")), "another seed, another train");
        assert_eq!(changes(&text.replace("seed = 7\n", "")), changes(&text.replace("seed = 7", "seed = 1")), "default seed 1");
    }

    #[test]
    fn bounce_errors() {
        let base = "[[bounce]]\npin = \"RB0\"\nlevel = 1\nbounces = 2\nperiod = 10\n";
        assert!(Stimulus::parse(base).is_ok());
        assert!(Stimulus::parse(&base.replace("period = 10", "period = [5, 5]\nseed = 3")).is_ok(), "a seed with a one-value range");
        let two = base.to_string() + "[[bounce]]\npin = \"RB0\"\nlevel = 0\nat = 41\nbounces = 1\nperiod = 5\n[[bounce]]\npin = \"RB1\"\nlevel = 0\nat = 0\nbounces = 1\nperiod = 5\n";
        assert!(Stimulus::parse(&two).is_ok(), "one after the other on a pin, or at once on two pins");
        for (bad, why) in [
            (base.replace("pin = \"RB0\"", "pin = \"RZ0\""), "pin"),
            (base.replace("level = 1", "level = 2"), "level"),
            (base.replace("bounces = 2", "bounces = 1001"), "bounces"),
            (base.replace("bounces = 2\n", ""), "no bounces"),
            (base.replace("period = 10\n", ""), "no period"),
            (base.replace("period = 10", "period = 0"), "period 0"),
            (base.replace("period = 10", "period = [9, 3]"), "reversed range"),
            (base.replace("period = 10", "period = [1, 2, 3]"), "three values"),
            (base.replace("period = 10", "period = 10\nseed = 3"), "seed without a range"),
            (base.to_string() + "[[bounce]]\npin = \"RB0\"\nlevel = 0\nat = 30\nbounces = 0\nperiod = 1\n", "overlapping trains on one pin"),
            (base.replace("period = 10", "period = 10\nfoo = 1"), "unknown key"),
            (base.replace("period = 10", "period = 4294967296\nat = 4294967296"), "past 2^32"),
        ] {
            assert!(Stimulus::parse(&bad).is_err(), "{why}");
        }
    }

    #[test]
    fn a_pin_entry_at_the_same_cycle_wins_over_a_bounce() {
        let pin = "[[pin]]\nport = \"B\"\nbit = 0\nlevel = 1\nat = 110\n";
        let bounce = "[[bounce]]\npin = \"RB0\"\nlevel = 1\nat = 100\nbounces = 1\nperiod = 10\n";
        for text in [format!("{pin}{bounce}"), format!("{bounce}{pin}")] {
            let st = Stimulus::parse(&text).unwrap();
            let mut d = crate::drive::Drive::default();
            for &(_, at, m, v) in &st.pins {
                d.push(at, 0, m, v);
            }
            assert_eq!(d.level(0, 110), 1, "the [[pin]] entry, not the bounce's 0, in either order");
        }
    }
}
