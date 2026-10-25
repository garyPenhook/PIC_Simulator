//! Device description: the data the compiler needs to know about one microcontroller.
//!
//! A description is generated from Microchip's device pack by `devgen` (never typed by hand) and
//! stored as TOML in `devices/`. The schema is documented in `docs/DEVICE-FORMAT.md`.
//! All addresses are plain integers in the data/program address space of the device; TOML has no
//! hex literal for tables written by serde, so tools should print them as decimal and humans can
//! use `printf '%x'`.

use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Device {
    pub device: Info,
    pub vectors: Vectors,
    pub memory: Memory,
    #[serde(default)]
    pub config: Vec<ConfigRegister>,
    #[serde(default)]
    pub sfr: Vec<Sfr>,
    #[serde(default)]
    pub joined: Vec<Joined>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Info {
    /// Canonical part name, e.g. `PIC18F27Q10`.
    pub name: String,
    /// Core family handled by a code-generator module, e.g. `pic18`.
    pub family: String,
    /// Architecture string from the pack, e.g. `18xxxx`.
    pub arch: String,
    pub pack: String,
    pub pack_version: String,
    /// File inside the pack this description was generated from.
    pub source: String,
    /// Data sheet the hand-entered values (vectors, cross-check table) come from.
    pub datasheet: String,
}

/// Fixed vector addresses. Not in the pack; entered by `devgen` from the data sheet per family.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Vectors {
    pub reset: u32,
    pub interrupt_high: u32,
    pub interrupt_low: u32,
    /// Where these numbers were read, e.g. `DS40002043F p.108`.
    pub source: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Region {
    pub start: u32,
    pub size: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeviceId {
    pub start: u32,
    pub size: u32,
    pub value: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Range {
    /// Inclusive start.
    pub start: u32,
    /// Exclusive end.
    pub end: u32,
}

impl Range {
    pub fn len(&self) -> u32 {
        self.end.saturating_sub(self.start)
    }
    pub fn is_empty(&self) -> bool {
        self.end <= self.start
    }
    pub fn contains(&self, addr: u32) -> bool {
        addr >= self.start && addr < self.end
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GprRange {
    pub bank: u8,
    pub start: u32,
    /// Exclusive end.
    pub end: u32,
    /// True for RAM the pack marks as system/sector RAM; not allocated to user variables.
    pub system: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Data {
    /// Sum of all GPR ranges, including system RAM (matches the data sheet's SRAM size).
    pub total_gpr: u32,
    /// GPR bytes the compiler may allocate (total minus system RAM).
    pub usable_gpr: u32,
    pub gpr: Vec<GprRange>,
    pub sfr: Vec<Range>,
    pub access_ram: Range,
    pub access_sfr: Range,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Memory {
    pub program: Region,
    pub eeprom: Region,
    pub user_id: Region,
    pub config: Region,
    pub device_id: DeviceId,
    pub data: Data,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Setting {
    pub name: String,
    pub value: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConfigField {
    pub name: String,
    pub pos: u8,
    pub width: u8,
    pub settings: Vec<Setting>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConfigRegister {
    pub name: String,
    pub addr: u32,
    /// Value of the register when left unprogrammed.
    pub default: u32,
    /// Mask of bits that exist.
    pub implemented: u32,
    pub fields: Vec<ConfigField>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Field {
    pub name: String,
    pub pos: u8,
    pub width: u8,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Sfr {
    pub name: String,
    pub addr: u32,
    /// Register width in bits (8 for ordinary SFRs).
    pub width: u8,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub aliases: Vec<String>,
    /// Bit fields using the current (data sheet) names.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fields: Vec<Field>,
    /// Bit fields using the legacy names (e.g. `TMR1ON`), when the pack provides them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub legacy: Vec<Field>,
}

/// A multi-byte register made of consecutive byte registers, least significant byte first
/// (`FSR0` = `FSR0L`,`FSR0H`). The number of *implemented* bits can be less than `bytes * 8`
/// (a 22-bit address register spans 3 bytes); the top member's field widths say how many.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Joined {
    pub name: String,
    /// Address of the least significant byte.
    pub addr: u32,
    pub bytes: u8,
    /// Byte registers, least significant first; member `i` is at `addr + i`.
    pub members: Vec<String>,
}

#[derive(Debug)]
pub enum Error {
    Io(std::io::Error),
    Parse(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Io(e) => write!(f, "{e}"),
            Error::Parse(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for Error {}

impl Device {
    pub fn load(path: &Path) -> Result<Device, Error> {
        let text = std::fs::read_to_string(path).map_err(Error::Io)?;
        Device::from_toml(&text)
    }

    pub fn from_toml(text: &str) -> Result<Device, Error> {
        toml::from_str(text).map_err(|e| Error::Parse(e.to_string()))
    }

    pub fn to_toml(&self) -> Result<String, Error> {
        toml::to_string(self).map_err(|e| Error::Parse(e.to_string()))
    }

    pub fn find_sfr(&self, name: &str) -> Option<&Sfr> {
        self.sfr
            .iter()
            .find(|s| s.name.eq_ignore_ascii_case(name) || s.aliases.iter().any(|a| a.eq_ignore_ascii_case(name)))
    }

    /// Number of SFRs whose name matches `pattern`, where `#` stands for one digit and every
    /// other character must match exactly (ASCII case-insensitive). `TX#STA` matches `TX1STA`.
    pub fn count_sfr(&self, pattern: &str) -> usize {
        self.sfr.iter().filter(|s| matches_pattern(&s.name, pattern)).count()
    }

    /// Internal consistency checks. Returns a list of problems; empty means the description is
    /// self-consistent (it does not prove the numbers are right — `devgen check` compares them with
    /// the data sheet).
    pub fn validate(&self) -> Vec<String> {
        let mut errs = Vec::new();
        let m = &self.memory;
        if m.program.size == 0 {
            errs.push("program memory size is zero".into());
        }
        // Parts with two interrupt priorities have reset < high < low; the enhanced mid-range
        // core has a single interrupt vector, recorded as high == low.
        let single_vector = self.device.family == "pic16e";
        let low_bad = if single_vector {
            self.vectors.interrupt_low != self.vectors.interrupt_high
        } else {
            self.vectors.interrupt_low <= self.vectors.interrupt_high
        };
        if self.vectors.interrupt_high <= self.vectors.reset || low_bad {
            errs.push(if single_vector {
                "vectors are not reset < interrupt, with high == low (single interrupt vector)".into()
            } else {
                "vectors are not in increasing order (reset < high < low)".into()
            });
        }
        // GPR ranges: sorted, non-empty, non-overlapping; totals agree.
        let mut total = 0u32;
        let mut usable = 0u32;
        let mut prev_end = 0u32;
        for (i, g) in m.data.gpr.iter().enumerate() {
            if g.end <= g.start {
                errs.push(format!("gpr range {i} is empty or reversed"));
            }
            if i > 0 && g.start < prev_end {
                errs.push(format!("gpr range {i} overlaps or is out of order"));
            }
            prev_end = g.end;
            total += g.end.saturating_sub(g.start);
            if !g.system {
                usable += g.end.saturating_sub(g.start);
            }
        }
        if total != m.data.total_gpr {
            errs.push(format!("total_gpr is {} but ranges sum to {}", m.data.total_gpr, total));
        }
        if usable != m.data.usable_gpr {
            errs.push(format!("usable_gpr is {} but non-system ranges sum to {}", m.data.usable_gpr, usable));
        }
        // SFRs: unique names, inside an SFR range, fields fit and do not overlap.
        let mut seen = std::collections::HashSet::new();
        for s in &self.sfr {
            if !seen.insert(s.name.to_ascii_uppercase()) {
                errs.push(format!("duplicate SFR name {}", s.name));
            }
            let bytes = u32::from(s.width).div_ceil(8).max(1);
            let in_sfr = m
                .data
                .sfr
                .iter()
                .chain(std::iter::once(&m.data.access_sfr))
                .any(|r| r.contains(s.addr) && r.contains(s.addr + bytes - 1));
            if !in_sfr {
                errs.push(format!("SFR {} at {:#x} is outside every SFR range", s.name, s.addr));
            }
            for (label, fields) in [("fields", &s.fields), ("legacy", &s.legacy)] {
                let mut used: u64 = 0;
                for f in fields.iter() {
                    if f.width == 0 || u32::from(f.pos) + u32::from(f.width) > u32::from(s.width) {
                        errs.push(format!("SFR {} {} {} does not fit in {} bits", s.name, label, f.name, s.width));
                        continue;
                    }
                    let mask = ((1u64 << f.width) - 1) << f.pos;
                    if used & mask != 0 {
                        errs.push(format!("SFR {} {} {} overlaps another field", s.name, label, f.name));
                    }
                    used |= mask;
                }
            }
        }
        for j in &self.joined {
            if j.members.len() != usize::from(j.bytes) || j.members.len() < 2 {
                errs.push(format!("joined register {} has {} members but declares {} bytes", j.name, j.members.len(), j.bytes));
            }
            for (i, member) in j.members.iter().enumerate() {
                match self.find_sfr(member) {
                    Some(s) if s.addr == j.addr + i as u32 => {}
                    Some(s) => errs.push(format!("joined {} member {} is at {:#x}, expected {:#x}", j.name, member, s.addr, j.addr + i as u32)),
                    None => errs.push(format!("joined {} member {} is not an SFR", j.name, member)),
                }
            }
        }
        // Config registers: inside the config region; field settings fit.
        for c in &self.config {
            if c.addr < m.config.start || c.addr >= m.config.start + m.config.size {
                errs.push(format!("config register {} at {:#x} is outside the config region", c.name, c.addr));
            }
            for f in &c.fields {
                let max = (1u64 << f.width) - 1;
                for s in &f.settings {
                    if u64::from(s.value) > max {
                        errs.push(format!("config {}.{} setting {} value {} does not fit {} bits", c.name, f.name, s.name, s.value, f.width));
                    }
                }
            }
        }
        errs
    }
}

fn matches_pattern(name: &str, pattern: &str) -> bool {
    let (n, p) = (name.as_bytes(), pattern.as_bytes());
    n.len() == p.len() && n.iter().zip(p).all(|(a, b)| if *b == b'#' { a.is_ascii_digit() } else { a.eq_ignore_ascii_case(b) })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tiny() -> Device {
        Device {
            device: Info {
                name: "TEST".into(),
                family: "pic18".into(),
                arch: "18xxxx".into(),
                pack: "p".into(),
                pack_version: "1".into(),
                source: "s".into(),
                datasheet: "d".into(),
            },
            vectors: Vectors { reset: 0, interrupt_high: 8, interrupt_low: 0x18, source: "t".into() },
            memory: Memory {
                program: Region { start: 0, size: 1024 },
                eeprom: Region { start: 0x31_0000, size: 256 },
                user_id: Region { start: 0x20_0000, size: 16 },
                config: Region { start: 0x30_0000, size: 4 },
                device_id: DeviceId { start: 0x3F_FFFE, size: 2, value: 1 },
                data: Data {
                    total_gpr: 0x200,
                    usable_gpr: 0x100,
                    gpr: vec![
                        GprRange { bank: 0, start: 0, end: 0x100, system: false },
                        GprRange { bank: 1, start: 0x100, end: 0x200, system: true },
                    ],
                    sfr: vec![Range { start: 0xF00, end: 0x1000 }],
                    access_ram: Range { start: 0, end: 0x60 },
                    access_sfr: Range { start: 0xF60, end: 0x1000 },
                },
            },
            config: vec![],
            sfr: vec![Sfr {
                name: "PORTB".into(),
                addr: 0xF8D,
                width: 8,
                aliases: vec![],
                fields: vec![Field { name: "RB0".into(), pos: 0, width: 1 }],
                legacy: vec![],
            }],
            joined: vec![],
        }
    }

    #[test]
    fn consistent_description_validates() {
        assert_eq!(tiny().validate(), Vec::<String>::new());
    }

    #[test]
    fn round_trips_through_toml() {
        let d = tiny();
        let text = d.to_toml().unwrap();
        assert_eq!(Device::from_toml(&text).unwrap(), d);
    }

    #[test]
    fn detects_wrong_totals_and_overlap() {
        let mut d = tiny();
        d.memory.data.total_gpr = 1;
        d.memory.data.gpr[1].start = 0x80;
        let errs = d.validate();
        assert!(errs.iter().any(|e| e.contains("total_gpr")), "{errs:?}");
        assert!(errs.iter().any(|e| e.contains("overlaps")), "{errs:?}");
    }

    #[test]
    fn detects_field_outside_register_and_overlap() {
        let mut d = tiny();
        d.sfr[0].fields = vec![
            Field { name: "A".into(), pos: 0, width: 4 },
            Field { name: "B".into(), pos: 3, width: 2 },
            Field { name: "C".into(), pos: 7, width: 2 },
        ];
        let errs = d.validate();
        assert!(errs.iter().any(|e| e.contains("B overlaps")), "{errs:?}");
        assert!(errs.iter().any(|e| e.contains("C does not fit")), "{errs:?}");
    }

    #[test]
    fn detects_sfr_outside_sfr_space() {
        let mut d = tiny();
        d.sfr[0].addr = 0x50;
        assert!(d.validate().iter().any(|e| e.contains("outside every SFR range")));
    }

    #[test]
    fn joined_members_must_be_consecutive_sfrs() {
        let mut d = tiny();
        for (n, a) in [("TMRL", 0xF90u32), ("TMRH", 0xF91)] {
            let mut s = d.sfr[0].clone();
            s.name = n.into();
            s.addr = a;
            d.sfr.push(s);
        }
        d.joined = vec![Joined { name: "TMR".into(), addr: 0xF90, bytes: 2, members: vec!["TMRL".into(), "TMRH".into()] }];
        assert_eq!(d.validate(), Vec::<String>::new());
        d.joined[0].members = vec!["TMRH".into(), "TMRL".into()];
        assert!(d.validate().iter().any(|e| e.contains("expected")));
    }

    #[test]
    fn pattern_counts_digits() {
        let mut d = tiny();
        for n in ["TX1STA", "TX2STA", "TXSTA", "TX12STA"] {
            let mut s = d.sfr[0].clone();
            s.name = n.into();
            s.addr = 0xF00 + d.sfr.len() as u32;
            d.sfr.push(s);
        }
        assert_eq!(d.count_sfr("TX#STA"), 2);
    }
}
