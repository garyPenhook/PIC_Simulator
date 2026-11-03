//! Configuration words: the fields the hex file programs (or the unprogrammed default), and the parts of the
//! reset state that depend on them.
//!
//! Facts, with sources, in `docs/FACTS-SIM.md` (section "Configuration words").

use devdesc::ConfigRegister;
use std::collections::BTreeMap;

/// Decoded configuration fields by name (`RSTOSC`, `WDTE`, ...).
#[derive(Debug, Clone, Default)]
pub struct Config {
    fields: BTreeMap<String, u32>,
}

impl Config {
    /// Decode every field of `regs` from the bytes the hex file put at the configuration addresses; a byte the
    /// image does not contain keeps the register's unprogrammed `default`.
    pub fn decode(regs: &[ConfigRegister], image: &BTreeMap<u32, u8>) -> Config {
        let mut fields = BTreeMap::new();
        for r in regs {
            let v = image.get(&r.addr).map_or(r.default, |&b| u32::from(b));
            for f in &r.fields {
                fields.insert(f.name.clone(), (v >> f.pos) & ((1u32 << f.width) - 1));
            }
        }
        Config { fields }
    }

    pub fn get(&self, name: &str) -> Option<u32> {
        self.fields.get(name).copied()
    }

    pub fn fields(&self) -> impl Iterator<Item = (&str, u32)> {
        self.fields.iter().map(|(k, v)| (k.as_str(), *v))
    }
}

/// SFR values at reset that come from the configuration: `(register, value)`.
///
/// * `OSCCON1` (NOSC/NDIV), `OSCCON2` (COSC/CDIV) and `OSCFRQ` follow `RSTOSC`: DS40002043F Table 5-1 p.53 (Q10),
///   DS40002214G Table 12-1 p.175 (Q41). The tables differ at `RSTOSC = 000`: NOSC/COSC is 110 on the Q10 and 000
///   on the Q41. Reserved codes (011, 001) change nothing.
/// * `WDTCON0` PS = `WDTCPS`, or 01011 when `WDTCPS` = 11111; SEN resets to 0: DS40002043F 10.8.1 p.102 and
///   DS40002214G 15.7.1 p.240.
/// * `WDTCON1` WINDOW = `WDTCWS`; CS = `WDTCCS`, or 000 when `WDTCCS` = 111: DS40002043F 10.8.2 p.103,
///   DS40002214G 15.7.2 p.241.
pub fn reset_sfrs(cfg: &Config, q41: bool) -> Vec<(&'static str, u8)> {
    let mut v = Vec::new();
    if let Some(rst) = cfg.get("RSTOSC") {
        let t = match (rst, q41) {
            (7, _) => Some((7, 0, 0)),
            (6, _) => Some((6, 2, 2)),
            (5, _) => Some((5, 0, 0)),
            (4, _) => Some((4, 0, 0)),
            (2, _) => Some((2, 0, 2)),
            (0, false) => Some((6, 0, 8)),
            (0, true) => Some((0, 0, 8)),
            _ => None,
        };
        if let Some((osc, div, frq)) = t {
            let b = osc << 4 | div;
            v.push(("OSCCON1", b));
            v.push(("OSCCON2", b));
            v.push(("OSCFRQ", frq));
        }
    }
    if let Some(cps) = cfg.get("WDTCPS") {
        let ps = if cps == 0x1F { 0x0B } else { cps as u8 };
        v.push(("WDTCON0", ps << 1));
    }
    if let (Some(cws), Some(ccs)) = (cfg.get("WDTCWS"), cfg.get("WDTCCS")) {
        let cs = if ccs == 7 { 0 } else { ccs as u8 };
        v.push(("WDTCON1", cs << 4 | cws as u8));
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(pairs: &[(&str, u32)]) -> Config {
        Config { fields: pairs.iter().map(|(k, v)| (k.to_string(), *v)).collect() }
    }

    #[test]
    fn rstosc_tables_differ_at_zero() {
        let c = cfg(&[("RSTOSC", 0)]);
        assert!(reset_sfrs(&c, false).contains(&("OSCCON1", 0x60)));
        assert!(reset_sfrs(&c, true).contains(&("OSCCON1", 0x00)));
        assert!(reset_sfrs(&c, true).contains(&("OSCFRQ", 8)));
    }

    #[test]
    fn one_mhz_is_four_mhz_divided_by_four() {
        let v = reset_sfrs(&cfg(&[("RSTOSC", 6)]), true);
        assert!(v.contains(&("OSCCON1", 0x62)) && v.contains(&("OSCCON2", 0x62)) && v.contains(&("OSCFRQ", 2)));
    }

    #[test]
    fn reserved_rstosc_changes_nothing() {
        assert!(reset_sfrs(&cfg(&[("RSTOSC", 3)]), false).is_empty());
    }

    #[test]
    fn watchdog_reset_values() {
        let v = reset_sfrs(&cfg(&[("WDTCPS", 0x1F), ("WDTCWS", 7), ("WDTCCS", 7)]), false);
        assert!(v.contains(&("WDTCON0", 0x16)) && v.contains(&("WDTCON1", 0x07)));
        let v = reset_sfrs(&cfg(&[("WDTCPS", 5), ("WDTCWS", 2), ("WDTCCS", 1)]), false);
        assert!(v.contains(&("WDTCON0", 0x0A)) && v.contains(&("WDTCON1", 0x12)));
    }
}
