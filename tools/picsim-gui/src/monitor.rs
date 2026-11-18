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

/// What the user asked of the Pins tab.
pub enum PinAction {
    /// Drive this input to the level.
    Drive(Pin, bool),
    /// Add the pin to the Waves capture.
    Trace(Pin),
}

/// A right-click menu with one item, "Trace <what> in Waves". True when it was chosen. The item is disabled (with the
/// reason) while a capture exists or is recording, because every sample holds one value per signal.
pub fn trace_menu(r: &egui::Response, what: &str, can_edit: bool) -> bool {
    let mut chosen = false;
    r.context_menu(|ui| {
        if ui.add_enabled(can_edit, egui::Button::new(format!("Trace {what} in Waves"))).on_disabled_hover_text("Clear the capture in the Waves tab first: the signal list cannot change while a capture exists").clicked() {
            chosen = true;
            ui.close();
        }
    });
    chosen
}

/// Adds the PORT bit of `pin` to the capture; the message says what happened.
pub fn trace_pin(d: &mut picsim::debugger::Debugger, dev: &Device, pin: Pin) -> String {
    let name = pin_name(pin);
    let Some(port) = sfr(dev, &format!("PORT{}", pin.0 as char)) else { return format!("{name}: this part has no PORT{} register", pin.0 as char) };
    let added = d.add_signal(picsim::debugger::Signal { name: name.clone(), addr: port.addr, bit: Some(pin.1) });
    if added { format!("{name} added to the Waves capture - press Record in the Waves tab") } else if !d.can_edit_signals() { format!("{name} not added: clear the capture first") } else { format!("{name} is already in the Waves capture") }
}

/// Adds a data byte (an SFR) to the capture; the message says what happened.
pub fn trace_byte(d: &mut picsim::debugger::Debugger, name: &str, addr: u32) -> String {
    if d.add_signal(picsim::debugger::Signal { name: name.to_string(), addr, bit: None }) {
        format!("{name} added to the Waves capture - press Record in the Waves tab")
    } else if !d.can_edit_signals() {
        format!("{name} not added: clear the capture first")
    } else {
        format!("{name} is already in the Waves capture")
    }
}

/// The Pins tab: every pin's level and direction at the current cycle. Returns a pin the user clicked to drive, with the
/// level to drive it to (only an input can be driven, and only when `can_drive`).
pub fn pins_ui(ui: &mut egui::Ui, pins: &[PinInfo], can_drive: bool, can_trace: bool, prev: &HashMap<Pin, bool>) -> Option<PinAction> {
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
                let r = r.on_hover_text(format!("{}: {state}{}\nright-click: trace in Waves", pin_name(p.pin), if !p.output && !p.analog && can_drive { "\nclick to drive it to the opposite level" } else { "" }));
                if r.clicked() && can_drive && !p.output && !p.analog {
                    out = Some(PinAction::Drive(p.pin, !p.high));
                }
                if trace_menu(&r, &pin_name(p.pin), can_trace) {
                    out = Some(PinAction::Trace(p.pin));
                }
            }
            ui.end_row();
        }
    });
    ui.add_space(4.0);
    ui.small("H/L: output high/low   1/0: input high/low   A: analog (digital input off)   orange outline: the level changed since the last Step or Run began");
    ui.small("Clicking an input drives it to the opposite level (when stopped); the drive lasts until overridden or Reset. Right-click a pin to add it to the Waves capture.");
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

/// Whether an interrupt whose flag and enable are set can actually be taken, from the global enables.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Gate {
    /// Nothing blocks it; the priority is named when the part has priority levels (`IPEN` = 1).
    Open(Option<&'static str>),
    /// The global enables that are clear and block it.
    Blocked(Vec<&'static str>),
}

/// A set interrupt flag, whether its enable is set, and (with the enable) what the global enables say.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pending {
    pub name: String,
    pub enable: Option<bool>,
    pub gate: Option<Gate>,
}

/// Address and position of a single-bit field by any of its names, in the named registers (`RCON` holds `IPEN` on the K22).
fn locate_bit(dev: &Device, regs: &[&str], names: &[&str]) -> Option<(u32, u8)> {
    for r in regs {
        let Some(s) = sfr(dev, r) else { continue };
        if let Some(f) = s.fields.iter().chain(&s.legacy).find(|f| f.width == 1 && f.pos < 8 && names.contains(&f.name.as_str())) {
            return Some((s.addr, f.pos));
        }
    }
    None
}

fn bit(dev: &Device, peek: &dyn Fn(u32) -> u8, regs: &[&str], names: &[&str]) -> Option<bool> {
    locate_bit(dev, regs, names).map(|(a, p)| peek(a) >> p & 1 != 0)
}

/// The global enables decide whether a flag whose enable is set interrupts (DS40002043F 15.13.1 p.198 and 15.13.2 p.199
/// Q10; DS40002214G 11.13.1 p.127 Q41; DS40001412H Reg. 9-1 p.109 K22; DS40002343F 12.10.1-2 p.114-115 and the other
/// PIC16 data sheets):
/// * `IPEN` = 0 (or none): `GIE` enables everything; `PEIE` also gates the sources of PIE1 and up (not PIE0, not the
///   INTCON sources; the Q41 has no `PEIE`).
/// * `IPEN` = 1: a high-priority source needs `GIEH`; a low-priority one needs `GIEH` and `GIEL`.
pub fn gate(dev: &Device, peek: &dyn Fn(u32) -> u8, enable_reg: &str, priority_high: bool) -> Gate {
    let core = ["INTCON", "INTCON0"];
    let ipen = bit(dev, peek, &["INTCON", "INTCON0", "RCON"], &["IPEN"]).unwrap_or(false);
    let gie = bit(dev, peek, &core, &["GIE_GIEH", "GIE", "GIEH"]);
    let bit6 = bit(dev, peek, &core, &["PEIE_GIEL", "PEIE", "GIEL"]);
    let vic = sfr(dev, "INTCON0").is_some();
    let peripheral = enable_reg.strip_prefix("PIE").is_some_and(|n| n != "0");
    let mut why = Vec::new();
    if gie == Some(false) {
        why.push(if ipen { "GIEH=0" } else { "GIE=0" });
    }
    if ipen {
        if !priority_high && bit6 == Some(false) {
            why.push("GIEL=0");
        }
        Gate::from_why(why, Some(if priority_high { "high priority" } else { "low priority" }))
    } else {
        if peripheral && !vic && bit6 == Some(false) {
            why.push("PEIE=0");
        }
        Gate::from_why(why, None)
    }
}

impl Gate {
    fn from_why(why: Vec<&'static str>, prio: Option<&'static str>) -> Gate {
        if why.is_empty() { Gate::Open(prio) } else { Gate::Blocked(why) }
    }
}

/// Every interrupt flag (`xxxIF`) that is set, with its enable (`xxxIE`) and, when the enable is set, the gate.
pub fn pending(dev: &Device, peek: &dyn Fn(u32) -> u8) -> Vec<Pending> {
    let mut enables: HashMap<String, (u32, u8, String)> = HashMap::new();
    let mut priorities: HashMap<String, (u32, u8)> = HashMap::new();
    for s in &dev.sfr {
        for f in s.fields.iter().chain(&s.legacy) {
            if f.width == 1 && f.name.len() > 2 && f.pos < 8 {
                if f.name.ends_with("IE") {
                    enables.entry(f.name.clone()).or_insert((s.addr, f.pos, s.name.clone()));
                } else if f.name.ends_with("IP") {
                    priorities.entry(f.name.clone()).or_insert((s.addr, f.pos));
                }
            }
        }
    }
    let mut out: Vec<Pending> = Vec::new();
    for s in &dev.sfr {
        let v = peek(s.addr);
        for f in s.fields.iter().chain(&s.legacy) {
            if f.width == 1 && f.name.len() > 2 && f.name.ends_with("IF") && f.pos < 8 && v >> f.pos & 1 != 0 && !out.iter().any(|p| p.name == f.name) {
                let stem = &f.name[..f.name.len() - 1];
                let en = enables.get(&format!("{stem}E"));
                let enable = en.map(|&(a, p, _)| peek(a) >> p & 1 != 0);
                // No priority bit (INT0 on the K22 is always high priority) reads as high.
                let high = priorities.get(&format!("{stem}P")).is_none_or(|&(a, p)| peek(a) >> p & 1 != 0);
                let gate = en.filter(|_| enable == Some(true)).map(|(_, _, reg)| gate(dev, peek, reg, high));
                out.push(Pending { name: f.name.clone(), enable, gate });
            }
        }
    }
    out.sort_by_key(|p| (!matches!(p.gate, Some(Gate::Open(_))), !p.enable.unwrap_or(false), p.name.clone()));
    out
}

/// State of the "fire an interrupt" list in the Peripherals tab.
#[derive(Default)]
pub struct Fire {
    pub filter: String,
    /// Firing also sets the flag's enable bit and the global enables that would block it.
    pub with_enables: bool,
}

/// A request to set or clear one interrupt flag.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FireRequest {
    pub name: String,
    pub set: bool,
    pub with_enables: bool,
}

/// Where a flag, its enable and its priority bit live.
struct Located {
    flag: (u32, u8),
    enable: Option<(u32, u8, String)>,
    priority: Option<(u32, u8)>,
}

fn locate(dev: &Device, flag: &str) -> Option<Located> {
    let stem = flag.strip_suffix("IF").filter(|s| !s.is_empty())?;
    let find = |name: String, want_reg: bool| {
        dev.sfr.iter().find_map(|s| s.fields.iter().chain(&s.legacy).find(|f| f.width == 1 && f.pos < 8 && f.name == name).map(|f| (s.addr, f.pos, want_reg.then(|| s.name.clone()).unwrap_or_default())))
    };
    let (fa, fp, _) = find(flag.to_string(), false)?;
    Some(Located { flag: (fa, fp), enable: find(format!("{stem}IE"), true), priority: find(format!("{stem}IP"), false).map(|(a, p, _)| (a, p)) })
}

/// Every interrupt flag the part has (`xxxIF`, one bit), by name, with its address and bit.
fn all_flags(dev: &Device) -> Vec<(String, u32, u8)> {
    let mut v: Vec<(String, u32, u8)> = Vec::new();
    for s in &dev.sfr {
        for f in s.fields.iter().chain(&s.legacy) {
            if f.width == 1 && f.pos < 8 && f.name.len() > 2 && f.name.ends_with("IF") && !v.iter().any(|x| x.0 == f.name) {
                v.push((f.name.clone(), s.addr, f.pos));
            }
        }
    }
    v.sort();
    v
}

/// The register writes (address, whole new byte) that carry out `req`, in the order to apply them: the enables first
/// and the flag last, so that the interrupt arrives with everything already enabled, as a hardware event would.
pub fn fire_writes(dev: &Device, peek: &dyn Fn(u32) -> u8, req: &FireRequest) -> Vec<(u32, u8)> {
    let Some(loc) = locate(dev, &req.name) else { return Vec::new() };
    let mut over: BTreeMap<u32, u8> = BTreeMap::new();
    let mut order: Vec<u32> = Vec::new();
    let mut put = |over: &mut BTreeMap<u32, u8>, addr: u32, pos: u8, on: bool| {
        let cur = over.get(&addr).copied().unwrap_or_else(|| peek(addr));
        over.insert(addr, if on { cur | 1 << pos } else { cur & !(1 << pos) });
        order.retain(|&a| a != addr);
        order.push(addr);
    };
    if req.set && req.with_enables {
        if let Some((a, p, reg)) = &loc.enable {
            put(&mut over, *a, *p, true);
            // Setting a blocker can reveal another (GIEH, then GIEL), so look again until nothing blocks it.
            for _ in 0..4 {
                let now = |a: u32| over.get(&a).copied().unwrap_or_else(|| peek(a));
                let high = loc.priority.is_none_or(|(pa, pp)| now(pa) >> pp & 1 != 0);
                let Gate::Blocked(why) = gate(dev, &now, reg, high) else { break };
                for w in why {
                    let names: &[&str] = match w {
                        "GIE=0" | "GIEH=0" => &["GIE_GIEH", "GIE", "GIEH"],
                        "PEIE=0" => &["PEIE_GIEL", "PEIE"],
                        _ => &["PEIE_GIEL", "GIEL"],
                    };
                    if let Some((ga, gp)) = locate_bit(dev, &["INTCON", "INTCON0"], names) {
                        put(&mut over, ga, gp, true);
                    }
                }
            }
        }
    }
    put(&mut over, loc.flag.0, loc.flag.1, req.set);
    // The flag's register goes last even when an enable shares it.
    order.retain(|&a| a != loc.flag.0);
    order.push(loc.flag.0);
    order.into_iter().map(|a| (a, over[&a])).collect()
}

/// What happened after `fire_writes` were applied (`peek` reads the machine now).
pub fn fire_report(dev: &Device, peek: &dyn Fn(u32) -> u8, req: &FireRequest) -> String {
    let Some(loc) = locate(dev, &req.name) else { return format!("{}: no such interrupt flag", req.name) };
    let is_set = peek(loc.flag.0) >> loc.flag.1 & 1 != 0;
    if is_set != req.set {
        return format!("{} did not {}: the bit is read-only or hardware-controlled on this part", req.name, if req.set { "set" } else { "clear" });
    }
    if !req.set {
        return format!("{} cleared", req.name);
    }
    match pending(dev, peek).iter().find(|p| p.name == req.name) {
        Some(Pending { gate: Some(Gate::Open(prio)), .. }) => format!("{} fired: it will interrupt{} on the next Step", req.name, prio.map_or(String::new(), |p| format!(" ({p})"))),
        Some(Pending { gate: Some(Gate::Blocked(why)), .. }) => format!("{} set, but blocked by {} (tick \"also enable it\" or set them)", req.name, why.join(" ")),
        Some(Pending { enable: Some(false), .. }) => format!("{} set, but its enable bit is clear (tick \"also enable it\" or set it)", req.name),
        _ => format!("{} set (it has no enable bit of that name, so it cannot interrupt)", req.name),
    }
}

/// The Peripherals tab: timers, EUSART, ADC and pending interrupt flags, decoded from the SFRs.
pub fn periph_ui(ui: &mut egui::Ui, dev: &Device, peek: &dyn Fn(u32) -> u8, prev: Option<&[u8]>, fire: &mut Fire, can_write: bool) -> Option<FireRequest> {
    let mut request = None;
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
            let ready = p.iter().filter(|x| matches!(x.gate, Some(Gate::Open(_)))).count();
            ui.label(format!("{} flag(s) set, {ready} can interrupt now", p.len()));
            for x in &p {
                let (txt, col) = match (&x.gate, x.enable) {
                    (Some(Gate::Open(prio)), _) => (format!("will interrupt{}", prio.map_or(String::new(), |p| format!(" ({p})"))), CHANGED),
                    (Some(Gate::Blocked(why)), _) => (format!("enabled, blocked by {}", why.join(" ")), egui::Color32::from_rgb(220, 120, 120)),
                    (None, Some(false)) => ("flag only (enable clear)".to_string(), ui.visuals().weak_text_color()),
                    (None, _) => ("flag only (no enable bit)".to_string(), ui.visuals().weak_text_color()),
                };
                ui.label(egui::RichText::new(format!("{:<10} {txt}", x.name)).monospace().color(col));
            }
            ui.separator();
            ui.label("Fire an interrupt");
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut fire.filter).hint_text("filter, e.g. TMR1").desired_width(110.0));
                ui.checkbox(&mut fire.with_enables, "also enable it").on_hover_text("also set the flag's enable bit and whichever of PEIE, GIE/GIEH and GIEL block it");
            });
            if !can_write {
                ui.small("Stop the simulator to fire or clear a flag.");
            }
            let f = fire.filter.trim().to_uppercase();
            egui::ScrollArea::vertical().id_salt("fire_list").max_height(150.0).show(ui, |ui| {
                for (name, addr, pos) in all_flags(dev).into_iter().filter(|(n, ..)| f.is_empty() || n.to_uppercase().contains(&f)) {
                    let set = peek(addr) >> pos & 1 != 0;
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new(format!("{name:<10} {}", if set { "1" } else { "0" })).monospace().color(if set { CHANGED } else { ui.visuals().text_color() }));
                        if ui.add_enabled(can_write && !set, egui::Button::new("Fire")).on_hover_text("set the flag (and the enables, if ticked); the interrupt is taken on the next Step if it is allowed").clicked() {
                            request = Some(FireRequest { name: name.clone(), set: true, with_enables: fire.with_enables });
                        }
                        if ui.add_enabled(can_write && set, egui::Button::new("Clear")).clicked() {
                            request = Some(FireRequest { name: name.clone(), set: false, with_enables: false });
                        }
                    });
                }
            });
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
    request
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

    /// A peek over the named registers; every other byte reads 0.
    fn regs(dev: &Device, set: &[(&str, u8)]) -> impl Fn(u32) -> u8 + use<> {
        let m: HashMap<u32, u8> = set.iter().map(|(n, v)| (dev.find_sfr(n).unwrap_or_else(|| panic!("no {n}")).addr, *v)).collect();
        move |a| m.get(&a).copied().unwrap_or(0)
    }

    fn find<'a>(p: &'a [Pending], n: &str) -> &'a Pending {
        p.iter().find(|x| x.name == n).unwrap_or_else(|| panic!("{n} not pending: {p:?}"))
    }

    #[test]
    fn pending_flags_pair_with_their_enables_on_real_parts() {
        for part in ["18F27Q10", "18F26K22", "16F18056", "18F16Q41"] {
            let dev = load(part);
            let p = pending(&dev, &|_| 0xFF);
            assert!(p.len() > 5, "{part}");
            assert_eq!(find(&p, "TMR1IF").enable, Some(true), "{part}");
            // Flags whose enable is set come before the rest.
            let first_flag_only = p.iter().position(|x| x.gate.is_none()).unwrap_or(p.len());
            assert!(p[first_flag_only..].iter().all(|x| x.gate.is_none()), "{part}: gated flags sort first");
        }
        // Every enable clear: flags are listed, none gated.
        let dev = load("18F27Q10");
        let p = pending(&dev, &regs(&dev, &[("PIR1", 0xFF)]));
        assert!(p.iter().all(|x| x.enable == Some(false) && x.gate.is_none()), "{p:?}");
    }

    #[test]
    fn q10_gates_follow_gie_peie_and_the_priority_bits() {
        let dev = load("18F27Q10");
        let all = |intcon: u8, ipr: u8| {
            let peek = regs(&dev, &[("INTCON", intcon), ("PIR0", 0xFF), ("PIE0", 0xFF), ("PIR1", 0xFF), ("PIE1", 0xFF), ("IPR0", ipr), ("IPR1", ipr)]);
            pending(&dev, &peek)
        };
        // IPEN = 0: GIE alone is enough for PIE0 sources; PIE1 sources also need PEIE.
        let p = all(0x00, 0xFF);
        assert_eq!(find(&p, "TMR0IF").gate, Some(Gate::Blocked(vec!["GIE=0"])));
        assert_eq!(find(&p, "ADIF").gate, Some(Gate::Blocked(vec!["GIE=0", "PEIE=0"])));
        let p = all(0x80, 0xFF);
        assert_eq!(find(&p, "TMR0IF").gate, Some(Gate::Open(None)));
        assert_eq!(find(&p, "ADIF").gate, Some(Gate::Blocked(vec!["PEIE=0"])));
        let p = all(0xC0, 0xFF);
        assert_eq!(find(&p, "ADIF").gate, Some(Gate::Open(None)));
        // IPEN = 1: high priority needs GIEH; low priority also GIEL.
        let p = all(0xA0, 0xFF);
        assert_eq!(find(&p, "ADIF").gate, Some(Gate::Open(Some("high priority"))));
        let p = all(0xA0, 0x00);
        assert_eq!(find(&p, "ADIF").gate, Some(Gate::Blocked(vec!["GIEL=0"])));
        let p = all(0xE0, 0x00);
        assert_eq!(find(&p, "ADIF").gate, Some(Gate::Open(Some("low priority"))));
        let p = all(0x60, 0xFF);
        assert_eq!(find(&p, "ADIF").gate, Some(Gate::Blocked(vec!["GIEH=0"])));
    }

    #[test]
    fn k22_keeps_ipen_in_rcon_and_int0_is_always_high_priority() {
        let dev = load("18F26K22");
        let peek = regs(&dev, &[("INTCON", 0x80 | 0x02 | 0x10), ("RCON", 0x80)]); // GIEH, INT0IF + INT0IE, IPEN = 1
        let p = pending(&dev, &peek);
        assert_eq!(find(&p, "INT0IF").gate, Some(Gate::Open(Some("high priority"))));
    }

    #[test]
    fn the_q41_has_no_peie_and_pic16_group_0_ignores_it() {
        let q41 = load("18F16Q41");
        let p = pending(&q41, &regs(&q41, &[("INTCON0", 0x80), ("PIR1", 0xFF), ("PIE1", 0xFF)]));
        assert!(p.iter().filter(|x| x.gate.is_some()).all(|x| x.gate == Some(Gate::Open(None))), "{p:?}");
        let p16 = load("16F17146");
        let p = pending(&p16, &regs(&p16, &[("INTCON", 0x80), ("PIR0", 0xFF), ("PIE0", 0xFF), ("PIR1", 0xFF), ("PIE1", 0xFF)]));
        assert_eq!(find(&p, "TMR0IF").gate, Some(Gate::Open(None)));
        assert_eq!(find(&p, "TMR1IF").gate, Some(Gate::Blocked(vec!["PEIE=0"])));
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
        let mut fire = Fire::default();
        for round in 0..2 {
            frame(&ctx, |ui| {
                let clicked = pins_ui(ui, &pins, true, true, &prev);
                assert!(clicked.is_none(), "{round}: nothing was clicked");
                assert!(periph_ui(ui, &dev, &|a| (a & 0xFF) as u8, Some(&snap), &mut fire, true).is_none());
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

    #[test]
    fn tracing_a_pin_or_register_adds_one_signal_and_says_why_it_cannot() {
        use picsim::debugger::{Debugger, Sample};
        let dev = load("18F27Q10");
        let mut d = Debugger::default();
        let port_b = dev.find_sfr("PORTB").unwrap().addr;
        assert!(trace_pin(&mut d, &dev, (b'B', 3)).contains("added"));
        assert_eq!((d.signals[0].addr, d.signals[0].bit, d.signals[0].name.as_str()), (port_b, Some(3), "RB3"));
        assert!(trace_pin(&mut d, &dev, (b'B', 3)).contains("already"));
        assert!(trace_byte(&mut d, "LATB", dev.find_sfr("LATB").unwrap().addr).contains("added"));
        assert!(trace_pin(&mut d, &dev, (b'Z', 0)).contains("no PORTZ"));
        d.samples.push(Sample { ns: 0, values: vec![0, 0] });
        assert!(trace_byte(&mut d, "TRISB", 1).contains("clear the capture"));
        assert_eq!(d.signals.len(), 2);
    }

    fn fire_q10(m: &mut picsim::Machine, name: &str, set: bool, with_enables: bool) -> String {
        let req = FireRequest { name: name.into(), set, with_enables };
        for (a, v) in fire_writes(&m.dev, &|a| m.cpu.peek(a), &req) {
            m.cpu.write(a, v);
        }
        m.cpu.settle();
        fire_report(&m.dev, &|a| m.cpu.peek(a), &req)
    }

    /// The part runs erased flash (NOPs), so reaching the vector address by counting proves nothing: an interrupt pushes the
    /// return address.
    fn taken(m: &mut picsim::Machine) -> bool {
        (0..40).any(|_| {
            m.step();
            m.cpu.stack_level() > 0
        })
    }

    #[test]
    fn firing_with_the_enables_takes_the_interrupt_on_a_real_q10() {
        let mut m = picsim::Machine::new(load("18F27Q10")).unwrap();
        let report = fire_q10(&mut m, "TMR1IF", true, true);
        assert!(report.contains("will interrupt"), "{report}");
        assert!(taken(&mut m), "the interrupt was not taken");
        assert_eq!(m.cpu.pc, m.dev.vectors.interrupt_high, "and it went to the high vector");
    }

    #[test]
    fn firing_without_the_enables_sets_the_flag_and_says_what_blocks_it() {
        let mut m = picsim::Machine::new(load("18F27Q10")).unwrap();
        let report = fire_q10(&mut m, "TMR1IF", true, false);
        assert!(report.contains("enable bit is clear"), "{report}");
        assert!(!taken(&mut m), "it must not interrupt");
        // The enable alone is not enough while GIE and PEIE are clear.
        let (enable_addr, ..) = locate(&m.dev, "TMR1IF").unwrap().enable.unwrap();
        m.cpu.write(enable_addr, 0xFF);
        let p = pending(&m.dev, &|a| m.cpu.peek(a));
        assert_eq!(find(&p, "TMR1IF").gate, Some(Gate::Blocked(vec!["GIE=0", "PEIE=0"])));
        // Clearing works and is reported.
        assert_eq!(fire_q10(&mut m, "TMR1IF", false, false), "TMR1IF cleared");
    }

    #[test]
    fn a_low_priority_source_with_ipen_gets_both_global_enables() {
        let mut m = picsim::Machine::new(load("18F27Q10")).unwrap();
        let w = |m: &mut picsim::Machine, n: &str, v: u8| m.cpu.write(m.dev.find_sfr(n).unwrap().addr, v);
        w(&mut m, "INTCON", 0x20); // IPEN = 1, no global enable
        w(&mut m, "IPR1", 0x00); // every PIE1 source low priority
        let report = fire_q10(&mut m, "ADIF", true, true);
        assert!(report.contains("low priority"), "{report}");
        let intcon = m.cpu.peek(m.dev.find_sfr("INTCON").unwrap().addr);
        assert_eq!(intcon & 0xC0, 0xC0, "GIEH and GIEL both set");
        assert!(taken(&mut m), "the low-priority interrupt was not taken");
        assert_eq!(m.cpu.pc, m.dev.vectors.interrupt_low, "and it went to the low vector");
    }

    #[test]
    fn a_read_only_flag_is_reported_not_silently_ignored() {
        // Hardware-controlled flags of the Q41 UART: writes do not set them.
        let mut m = picsim::Machine::new(load("18F16Q41")).unwrap();
        let report = fire_q10(&mut m, "U1FERIF", true, false);
        assert!(report.contains("did not set"), "{report}");
        assert!(fire_writes(&m.dev, &|a| m.cpu.peek(a), &FireRequest { name: "NOPE".into(), set: true, with_enables: true }).is_empty());
    }

    #[test]
    fn firing_works_on_a_pic16_too() {
        let mut m = picsim::machine16::Machine16::new(load("16F17146")).unwrap();
        let vector = m.dev.vectors.interrupt_high;
        let req = FireRequest { name: "TMR1IF".into(), set: true, with_enables: true };
        for (a, v) in fire_writes(&m.dev, &|a| m.cpu.peek(a), &req) {
            m.cpu.write(a, v);
        }
        m.cpu.settle();
        let report = fire_report(&m.dev, &|a| m.cpu.peek(a), &req);
        assert!(report.contains("will interrupt"), "{report}");
        assert!((0..40).any(|_| { m.step(); m.cpu.depth() > 0 }), "the interrupt was not taken");
        assert_eq!(m.cpu.pc, vector, "and it went to the vector");
    }
}
