//! Monitoring views: the live pin overview, change highlighting helpers and the decoded peripheral view.
use devdesc::{Device, Field, Sfr};
use eframe::egui;
use picsim::periph::{Pin, PinInfo, pin_name};
use std::collections::{BTreeMap, HashMap};

/// A byte that changed since the last Step or Run began.
pub const CHANGED: egui::Color32 = egui::Color32::from_rgb(255, 170, 60);

/// The data tabs, in the order of `App::tab`.
pub const TABS: [&str; 10] = ["Memory", "SFRs", "Watch", "Board", "Variables", "UART", "Debugger", "Pins", "Peripherals", "Waves"];

/// One row of the Memory tab: address, 16 bytes (`None` = not implemented) and the ASCII column. A byte that differs from
/// `prev` (the view when the last Step/Run began) is orange.
pub fn memory_row(ui: &mut egui::Ui, a: usize, bytes: &[Option<u8>; 16], prev: Option<&[u8]>) {
    let font = egui::TextStyle::Monospace.resolve(ui.style());
    let plain = ui.visuals().text_color();
    let mut job = egui::text::LayoutJob::default();
    let mut add = |t: String, c: egui::Color32| job.append(&t, 0.0, egui::TextFormat { font_id: font.clone(), color: c, ..Default::default() });
    add(format!("{a:04X}  "), plain);
    for (i, b) in bytes.iter().enumerate() {
        let changed = b.is_some_and(|b| prev.and_then(|p| p.get(a + i)).is_some_and(|&p| p != b));
        add(b.map_or("--".to_string(), |b| format!("{b:02X}")), if changed { CHANGED } else { plain });
        add(" ".into(), plain);
    }
    add(" ".into(), plain);
    for (i, b) in bytes.iter().enumerate() {
        let changed = b.is_some_and(|b| prev.and_then(|p| p.get(a + i)).is_some_and(|&p| p != b));
        add(b.map_or('.', |b| if (32..127).contains(&b) { b as char } else { '.' }).to_string(), if changed { CHANGED } else { plain });
    }
    ui.label(job);
}

/// The Pins tab: every pin's level and direction at the current cycle. Returns a pin the user clicked to drive, with the
/// level to drive it to (only an input can be driven, and only when `can_drive`).
pub fn pins_ui(ui: &mut egui::Ui, pins: &[PinInfo], can_drive: bool, prev: &HashMap<Pin, bool>) -> Option<(Pin, bool)> {
    let mut out = None;
    ui.label("Level and direction of every pin at the current cycle.");
    let by_port: BTreeMap<u8, Vec<&PinInfo>> = pins.iter().fold(BTreeMap::new(), |mut m, p| {
        m.entry(p.pin.0).or_default().push(p);
        m
    });
    let dark = ui.visuals().dark_mode;
    let (out_hi, out_lo, in_hi, in_lo, ana) = if dark {
        (egui::Color32::from_rgb(200, 70, 50), egui::Color32::from_rgb(90, 40, 40), egui::Color32::from_rgb(40, 130, 60), egui::Color32::from_rgb(45, 55, 50), egui::Color32::from_rgb(70, 70, 110))
    } else {
        (egui::Color32::from_rgb(240, 120, 100), egui::Color32::from_rgb(200, 160, 150), egui::Color32::from_rgb(110, 200, 130), egui::Color32::from_rgb(200, 215, 205), egui::Color32::from_rgb(170, 170, 220))
    };
    egui::Grid::new("pin_grid").num_columns(9).spacing([3.0, 3.0]).show(ui, |ui| {
        ui.label("");
        for b in (0..8).rev() {
            ui.label(egui::RichText::new(b.to_string()).weak());
        }
        ui.end_row();
        for (letter, list) in &by_port {
            ui.label(egui::RichText::new(format!("R{}", *letter as char)).strong());
            for b in (0..8u8).rev() {
                let Some(p) = list.iter().find(|p| p.pin.1 == b) else {
                    ui.label("");
                    continue;
                };
                let (fill, text) = match (p.analog, p.output, p.high) {
                    (true, _, _) => (ana, "A"),
                    (_, true, true) => (out_hi, "H"),
                    (_, true, false) => (out_lo, "L"),
                    (_, false, true) => (in_hi, "1"),
                    (_, false, false) => (in_lo, "0"),
                };
                let flipped = prev.get(&p.pin).is_some_and(|&h| h != p.high);
                let mut button = egui::Button::new(egui::RichText::new(text).monospace()).fill(fill);
                if flipped {
                    button = button.stroke(egui::Stroke::new(2.0, CHANGED));
                }
                let r = ui.add_sized([26.0, 22.0], button);
                let state = if p.analog { "analog" } else if p.output { if p.high { "output, high" } else { "output, low" } } else if p.high { "input, high" } else { "input, low" };
                let r = r.on_hover_text(format!("{}: {state}{}", pin_name(p.pin), if !p.output && !p.analog && can_drive { "\nclick to drive it to the opposite level" } else { "" }));
                if r.clicked() && can_drive && !p.output && !p.analog {
                    out = Some((p.pin, !p.high));
                }
            }
            ui.end_row();
        }
    });
    ui.add_space(4.0);
    ui.small("H/L: output high/low   1/0: input high/low   A: analog (digital input off)   orange outline: the level changed since the last Step or Run began");
    ui.small("Clicking an input drives it to the opposite level (when stopped); the drive lasts until overridden or Reset.");
    out
}

fn sfr<'a>(dev: &'a Device, name: &str) -> Option<&'a Sfr> {
    dev.sfr.iter().find(|s| s.name == name || s.aliases.iter().any(|a| a == name))
}

/// Value of an SFR or of a joined multi-byte register (`TMR1` = `TMR1L`,`TMR1H`).
fn value(dev: &Device, peek: &dyn Fn(u32) -> u8, name: &str) -> Option<(u64, u8)> {
    if let Some(j) = dev.joined.iter().find(|j| j.name == name) {
        let mut v = 0u64;
        for (i, m) in j.members.iter().enumerate() {
            v |= u64::from(peek(sfr(dev, m)?.addr)) << (8 * i);
        }
        return Some((v, j.bytes));
    }
    sfr(dev, name).map(|s| (u64::from(peek(s.addr)), 1))
}

/// Single-bit fields that are set and multi-bit fields that are not zero, by name.
fn field_list(s: &Sfr, v: u8) -> String {
    let mut seen: Vec<&Field> = Vec::new();
    for f in s.fields.iter().chain(&s.legacy) {
        if !seen.iter().any(|g| g.pos == f.pos && g.width == f.width) && !seen.iter().any(|g| g.name == f.name) {
            seen.push(f);
        }
    }
    seen.sort_by_key(|f| std::cmp::Reverse(f.pos));
    seen.iter()
        .filter(|f| f.pos < 8)
        .filter_map(|f| {
            let x = (u32::from(v) >> f.pos) & ((1u32 << f.width.min(8)) - 1);
            match (f.width, x) {
                (_, 0) => None,
                (1, _) => Some(f.name.clone()),
                _ => Some(format!("{}={x:X}", f.name)),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn reg_line(ui: &mut egui::Ui, s: &Sfr, v: u8, prev: Option<&[u8]>) {
    let changed = prev.and_then(|p| p.get(s.addr as usize)).is_some_and(|&p| p != v);
    let col = if changed { CHANGED } else { ui.visuals().text_color() };
    ui.horizontal_wrapped(|ui| {
        ui.label(egui::RichText::new(format!("{:<9} {v:02X}", s.name)).monospace().color(col));
        let f = field_list(s, v);
        if !f.is_empty() {
            ui.label(egui::RichText::new(f).monospace().weak());
        }
    });
}

fn digits(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_digit())
}

/// `RC1STA`, `TXSTA2`, `BAUD1CON`, `SP1BRGL`, `U1CON0`: the registers of an EUSART.
fn is_uart(n: &str) -> bool {
    let strip = |p: &str| n.strip_prefix(p);
    let numbered = |rest: &str, tail: &str| {
        let d: String = rest.chars().take_while(char::is_ascii_digit).collect();
        let r = &rest[d.len()..];
        d.len() <= 1 && r == tail
    };
    // RCxSTA / RCSTAx, TXxSTA / TXSTAx
    for p in ["RC", "TX"] {
        if let Some(r) = strip(p) {
            if numbered(r, "STA") {
                return true;
            }
            if let Some(d) = r.strip_prefix("STA") && (d.is_empty() || digits(d)) {
                return true;
            }
        }
    }
    if let Some(r) = strip("BAUD") && (numbered(r, "CON") || r.strip_prefix("CON").is_some_and(|d| d.is_empty() || digits(d))) {
        return true;
    }
    if let Some(r) = strip("SP") {
        let d: String = r.chars().take_while(char::is_ascii_digit).collect();
        if matches!(&r[d.len()..], "BRG" | "BRGL" | "BRGH") {
            return true;
        }
    }
    if let Some(r) = strip("U") {
        let d: String = r.chars().take_while(char::is_ascii_digit).collect();
        if d.len() == 1 && matches!(&r[1..], "CON0" | "CON1" | "CON2" | "BRG" | "BRGL" | "BRGH" | "ERRIR" | "ERRIE" | "UIR" | "FIFO" | "P1" | "P2" | "P3" | "TXCHK" | "RXB" | "TXB") {
            return true;
        }
    }
    false
}

/// ADC control and result registers (`ADCON0`, `ADRESH`, `ADPCH` ...).
fn is_adc(n: &str) -> bool {
    n.starts_with("AD") && !n.starts_with("ADDR")
}

/// Registers of Timer `n`: `T1CON`, `T1GCON`, `T0CON0`, `T2CLK` ... (not the next timer's: `T1` then a digit is Timer 10+).
fn is_timer_reg(n: &str, t: &str) -> bool {
    n.strip_prefix(&format!("T{t}")).and_then(|r| r.chars().next()).is_some_and(|c| c.is_ascii_alphabetic()) && !n.starts_with("TMR")
}

/// Flags of the form `xxxIF`, with whether the matching `xxxIE` is set, for every interrupt flag that is set.
fn pending(dev: &Device, peek: &dyn Fn(u32) -> u8) -> Vec<(String, Option<bool>)> {
    let mut enables: HashMap<String, (u32, u8)> = HashMap::new();
    for s in &dev.sfr {
        for f in s.fields.iter().chain(&s.legacy) {
            if f.width == 1 && f.name.len() > 2 && f.name.ends_with("IE") {
                enables.entry(f.name.clone()).or_insert((s.addr, f.pos));
            }
        }
    }
    let mut out: Vec<(String, Option<bool>)> = Vec::new();
    for s in &dev.sfr {
        let v = peek(s.addr);
        for f in s.fields.iter().chain(&s.legacy) {
            if f.width == 1 && f.name.len() > 2 && f.name.ends_with("IF") && f.pos < 8 && v >> f.pos & 1 != 0 && !out.iter().any(|(n, _)| *n == f.name) {
                let en = enables.get(&format!("{}E", &f.name[..f.name.len() - 1])).map(|&(a, p)| peek(a) >> p & 1 != 0);
                out.push((f.name.clone(), en));
            }
        }
    }
    out.sort_by_key(|(n, e)| (!e.unwrap_or(false), n.clone()));
    out
}

/// The Peripherals tab: timers, EUSART, ADC and pending interrupt flags, decoded from the SFRs.
pub fn periph_ui(ui: &mut egui::Ui, dev: &Device, peek: &dyn Fn(u32) -> u8, prev: Option<&[u8]>) {
    egui::ScrollArea::vertical().show(ui, |ui| {
        ui.label("Decoded from the special function registers. Orange: changed since the last Step or Run began.");
        egui::CollapsingHeader::new("Interrupts").default_open(true).show(ui, |ui| {
            let mut gates = Vec::new();
            for n in ["GIE", "GIEH", "GIEL", "PEIE", "IPEN"] {
                for s in &dev.sfr {
                    if let Some(f) = s.fields.iter().chain(&s.legacy).find(|f| f.name == n && f.width == 1) {
                        gates.push(format!("{n}={}", peek(s.addr) >> f.pos & 1));
                        break;
                    }
                }
            }
            if !gates.is_empty() {
                ui.monospace(gates.join("  "));
            }
            let p = pending(dev, peek);
            let ready = p.iter().filter(|(_, e)| *e == Some(true)).count();
            ui.label(format!("{} flag(s) set, {ready} with their enable set (the global enables above still decide whether it interrupts)", p.len()));
            for (n, e) in &p {
                let (txt, col) = match e {
                    Some(true) => ("flag and enable set", CHANGED),
                    Some(false) => ("flag only (enable clear)", ui.visuals().weak_text_color()),
                    None => ("flag only (no enable bit)", ui.visuals().weak_text_color()),
                };
                ui.label(egui::RichText::new(format!("{n:<10} {txt}")).monospace().color(col));
            }
        });
        egui::CollapsingHeader::new("Timers").default_open(true).show(ui, |ui| {
            let mut ids: Vec<String> = dev.sfr.iter().filter_map(|s| s.name.strip_prefix("TMR").filter(|r| digits(r)).map(str::to_string)).collect();
            ids.extend(dev.joined.iter().filter_map(|j| j.name.strip_prefix("TMR").filter(|r| digits(r)).map(str::to_string)));
            ids.sort_by_key(|i| i.parse::<u32>().unwrap_or(0));
            ids.dedup();
            if ids.is_empty() {
                ui.label("none");
            }
            for id in ids {
                let Some((v, bytes)) = value(dev, peek, &format!("TMR{id}")) else { continue };
                let tmr_changed = prev.is_some_and(|p| {
                    let addrs: Vec<u32> = match dev.joined.iter().find(|j| j.name == format!("TMR{id}")) {
                        Some(j) => j.members.iter().filter_map(|m| sfr(dev, m).map(|s| s.addr)).collect(),
                        None => sfr(dev, &format!("TMR{id}")).map(|s| s.addr).into_iter().collect(),
                    };
                    addrs.iter().any(|&a| p.get(a as usize).is_some_and(|&b| b != peek(a)))
                });
                let col = if tmr_changed { CHANGED } else { ui.visuals().text_color() };
                let w = usize::from(bytes) * 2;
                let mut line = format!("TMR{id:<3} {v:0w$X}  ({v})");
                if let Some((pr, _)) = value(dev, peek, &format!("PR{id}")) {
                    line.push_str(&format!("   PR{id} = {pr:02X}"));
                }
                ui.label(egui::RichText::new(line).monospace().strong().color(col));
                for s in dev.sfr.iter().filter(|s| is_timer_reg(&s.name, &id)) {
                    reg_line(ui, s, peek(s.addr), prev);
                }
            }
        });
        let group = |ui: &mut egui::Ui, title: &str, pick: &dyn Fn(&str) -> bool, joined: &[&str]| {
            egui::CollapsingHeader::new(title).default_open(true).show(ui, |ui| {
                let mut any = false;
                for j in joined {
                    if let Some((v, bytes)) = value(dev, peek, j) {
                        any = true;
                        ui.monospace(format!("{j:<9} {v:0w$X}  ({v})", w = usize::from(bytes) * 2));
                    }
                }
                for s in dev.sfr.iter().filter(|s| pick(&s.name)) {
                    any = true;
                    reg_line(ui, s, peek(s.addr), prev);
                }
                if !any {
                    ui.label("none on this part");
                }
            });
        };
        group(ui, "EUSART", &is_uart, &["SP1BRG", "SP2BRG", "U1BRG", "U2BRG"]);
        group(ui, "ADC", &is_adc, &["ADRES", "ADRES0"]);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uart_registers_are_recognised_on_both_naming_schemes() {
        for n in ["RC1STA", "RCSTA", "RCSTA1", "TX2STA", "TXSTA", "BAUD1CON", "BAUDCON", "BAUDCON1", "SP1BRGL", "SPBRG", "SPBRGH", "SP2BRGH", "U1CON0", "U2BRGL", "U1ERRIR"] {
            assert!(is_uart(n), "{n}");
        }
        for n in ["RCON", "TXREG1", "RCREG", "SSP1CON1", "TRISC", "TMR1", "PORTC", "RC1PPS"] {
            assert!(!is_uart(n), "{n}");
        }
    }

    #[test]
    fn a_timer_owns_only_its_own_control_registers() {
        assert!(is_timer_reg("T1CON", "1") && is_timer_reg("T1GCON", "1") && is_timer_reg("T0CON0", "0"));
        assert!(!is_timer_reg("T10CON", "1") && !is_timer_reg("TMR1", "1") && !is_timer_reg("T2CON", "1"));
    }

    #[test]
    fn adc_registers_exclude_address_registers() {
        assert!(is_adc("ADCON0") && is_adc("ADRESH") && !is_adc("ADDRESS") && !is_adc("TRISA"));
    }

    #[test]
    fn field_list_names_the_bits_that_are_set() {
        let s = Sfr {
            name: "X".into(), addr: 0, width: 8, por: String::new(), mclr: String::new(), access: String::new(), implemented: Some(255), module: String::new(), aliases: vec![],
            fields: vec![Field { name: "ON".into(), pos: 7, width: 1 }, Field { name: "CKPS".into(), pos: 4, width: 2 }, Field { name: "OFF".into(), pos: 0, width: 1 }],
            legacy: vec![Field { name: "TMRON".into(), pos: 7, width: 1 }],
        };
        assert_eq!(field_list(&s, 0b1010_0000), "ON CKPS=2");
        assert_eq!(field_list(&s, 0), "");
    }

    fn load(part: &str) -> Device {
        Device::load(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("../../devices/{part}.toml"))).unwrap()
    }

    #[test]
    fn pending_flags_pair_with_their_enables_on_real_parts() {
        for part in ["18F27Q10", "18F26K22", "16F18056"] {
            let dev = load(part);
            // Every flag set, every enable clear: nothing is pending, but flags are listed.
            let sfr_ie: Vec<u32> = dev.sfr.iter().filter(|s| s.name.starts_with("PIE") || s.name == "INTCON").map(|s| s.addr).collect();
            let peek = |a: u32| if sfr_ie.contains(&a) { 0 } else { 0xFF };
            let p = pending(&dev, &peek);
            assert!(!p.is_empty(), "{part}");
            assert!(p.iter().any(|(n, _)| n == "TMR1IF"), "{part}: {p:?}");
            assert!(p.iter().filter_map(|(_, e)| *e).all(|e| !e) || part == "18F26K22", "{part}: no enable should read set");
            // Enables set too: TMR1IF is pending, and pending flags sort first.
            let all = pending(&dev, &|_| 0xFF);
            assert_eq!(all.iter().find(|(n, _)| n == "TMR1IF").map(|x| x.1), Some(Some(true)), "{part}");
            assert!(all.first().is_some_and(|(_, e)| *e == Some(true)), "{part}");
        }
    }

    #[test]
    fn joined_timer_values_read_little_endian_and_unknown_names_are_none() {
        let dev = load("18F27Q10");
        let l = dev.find_sfr("TMR1L").unwrap().addr;
        let h = dev.find_sfr("TMR1H").unwrap().addr;
        let peek = |a: u32| if a == l { 0x34 } else if a == h { 0x12 } else { 0 };
        assert_eq!(value(&dev, &peek, "TMR1"), Some((0x1234, 2)));
        assert_eq!(value(&dev, &peek, "NOPE"), None);
    }

    /// One headless frame; the texture upload nobody applies is dropped on purpose.
    fn frame(ctx: &egui::Context, f: impl FnMut(&mut egui::Ui)) {
        ctx.run_ui(egui::RawInput::default(), f).textures_delta.clear();
    }

    #[test]
    fn the_views_draw_without_panicking() {
        let dev = load("18F27Q10");
        let pins: Vec<PinInfo> = (0..8).map(|b| PinInfo { pin: (b'C', b), output: b % 2 == 0, high: b % 3 == 0, analog: b == 5 }).collect();
        let prev: HashMap<Pin, bool> = pins.iter().map(|p| (p.pin, !p.high)).collect();
        let snap = vec![0u8; 0x1000];
        let ctx = egui::Context::default();
        for round in 0..2 {
            frame(&ctx, |ui| {
                let clicked = pins_ui(ui, &pins, true, &prev);
                assert!(clicked.is_none(), "{round}: nothing was clicked");
                periph_ui(ui, &dev, &|a| (a & 0xFF) as u8, Some(&snap));
                memory_row(ui, 0x10, &std::array::from_fn(|i| (i % 5 != 0).then_some(i as u8 * 17)), Some(&snap));
            });
        }
    }

    #[test]
    fn the_waveform_draws_a_capture_of_bits_and_bytes() {
        use picsim::debugger::{Debugger, Sample, Signal};
        let mut d = Debugger::default();
        d.signals = vec![Signal { name: "RB0".into(), addr: 0, bit: Some(0) }, Signal { name: "LATC".into(), addr: 1, bit: None }];
        d.samples = (0..200u64).map(|i| Sample { ns: i * 250, values: vec![(i / 7 % 2) as u8, (i * 3) as u8] }).collect();
        let mut tools = crate::debugger::Tools::default();
        let ctx = egui::Context::default();
        for _ in 0..3 {
            frame(&ctx, |ui| tools.waves(ui, &mut d, true));
        }
        d.samples.truncate(1); // a one-sample capture has no duration
        frame(&ctx, |ui| tools.waves(ui, &mut d, true));
        d.samples.clear();
        frame(&ctx, |ui| tools.waves(ui, &mut d, true));
    }
}
