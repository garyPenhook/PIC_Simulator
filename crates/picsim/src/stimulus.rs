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
//! ```
//! Pins read 0 until driven. Only input pins (TRISx = 1) with ANSELx = 0 show the level; an output pin shows its latch.

use crate::periph::RxStream;
use devdesc::Device;

/// Largest `at`, `bit` and `gap`: 2^32 instruction cycles. Keeps the receiver's cycle arithmetic far from u64 overflow.
pub const MAX_CYCLES: u64 = 1 << 32;

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Stimulus {
    pub uart: Vec<(u8, RxStream)>,
    /// (channel, from cycle, code)
    pub adc: Vec<(u8, u64, u16)>,
    /// (port letter, from cycle, pin mask, levels)
    pub pins: Vec<(u8, u64, u8, u8)>,
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
