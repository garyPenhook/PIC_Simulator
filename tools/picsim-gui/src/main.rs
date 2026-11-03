//! `basil-sim-gui [prog.hex] [--device 18F16Q41]`: run, step and inspect a program on a simulated PIC18.

use eframe::egui;
use picsim::periph::{Board, Bus, Led, Oled, Pin, pin_name};
use picsim::{Machine, Stop, disasm::disasm};
use std::collections::BTreeMap;
use std::path::PathBuf;

struct App {
    devices_dir: PathBuf,
    devices: Vec<String>,
    device: String,
    hex_path: String,
    m: Option<Machine>,
    msg: String,
    running: bool,
    speed: f32, // log10 of cycles per frame
    fosc_mhz: f32,
    view_addr: u32,
    follow: bool,
    tab: usize,
    mem_filter: String,
    watch: Vec<(String, usize)>,
    watch_in: String,
    goto_in: String,
    labels: BTreeMap<u32, String>,
    last_stop: Option<Stop>,
    stim_path: String,
    /// LEDs and OLED wired in the Board tab, kept when another program is loaded.
    board_cfg: Board,
    led_duty: Vec<f32>,
}

fn list_devices(dir: &PathBuf) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| n.ends_with(".toml") && !n.ends_with(".expect.toml"))
        .map(|n| n.trim_end_matches(".toml").to_string())
        .filter(|n| n.starts_with("18F"))
        .collect();
    v.sort();
    v
}

impl App {
    fn new(hex: Option<String>, device: Option<String>) -> App {
        let devices_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../devices");
        let devices = list_devices(&devices_dir);
        let mut a = App {
            devices_dir,
            device: device.or_else(|| devices.first().cloned()).unwrap_or_default(),
            devices,
            hex_path: hex.clone().unwrap_or_default(),
            m: None,
            msg: String::new(),
            running: false,
            speed: 4.0,
            fosc_mhz: 64.0,
            view_addr: 0,
            follow: true,
            tab: 0,
            mem_filter: String::new(),
            watch: Vec::new(),
            watch_in: String::new(),
            goto_in: String::new(),
            labels: BTreeMap::new(),
            last_stop: None,
            stim_path: String::new(),
            board_cfg: Board::default(),
            led_duty: Vec::new(),
        };
        if hex.is_some() {
            a.load();
        }
        a
    }

    fn load(&mut self) {
        self.running = false;
        self.last_stop = None;
        let res = (|| -> Result<Machine, String> {
            let dev = devdesc::Device::load(&self.devices_dir.join(format!("{}.toml", self.device))).map_err(|e| e.to_string())?;
            let mut m = Machine::new(dev)?;
            let p = PathBuf::from(&self.hex_path);
            m.load_hex(&std::fs::read_to_string(&p).map_err(|e| format!("{}: {e}", p.display()))?)?;
            for ext in ["S", "_basil.S"] {
                let asm = if ext == "S" { p.with_extension("S") } else { p.with_file_name(format!("{}_basil.S", p.file_stem().unwrap_or_default().to_string_lossy())) };
                if let Ok(t) = std::fs::read_to_string(asm) {
                    m.load_symbols_from_asm(&t);
                    break;
                }
            }
            if !self.stim_path.trim().is_empty() {
                let f = PathBuf::from(self.stim_path.trim());
                m.load_stimulus(&std::fs::read_to_string(&f).map_err(|e| format!("{}: {e}", f.display()))?).map_err(|e| format!("{}: {e}", f.display()))?;
            }
            Ok(m)
        })();
        match res {
            Ok(m) => {
                self.labels.clear();
                for (n, a) in &m.syms {
                    if m.cpu.prog.get(*a as usize).is_some_and(|b| *b != 0xFF) && !n.starts_with("T_") {
                        self.labels.entry(*a).or_insert_with(|| n.clone());
                    }
                }
                let mut m = m;
                if m.cpu.p.board.is_empty() {
                    m.cpu.p.set_board(self.board_cfg.fresh()); // keep the parts wired in an earlier session
                } else {
                    self.board_cfg = m.cpu.p.board.fresh(); // the stimulus file wired them
                }
                self.msg = format!("loaded {} ({} symbols)", self.hex_path, m.syms.len());
                self.m = Some(m);
            }
            Err(e) => {
                self.msg = e;
                self.m = None;
            }
        }
    }

    fn advance(&mut self, cycles: u64) {
        if let Some(m) = &mut self.m {
            let halt = m.syms.get("__halt").copied();
            let stop = m.run(halt, cycles);
            match stop {
                Stop::Cycles => {}
                s => {
                    self.running = false;
                    self.last_stop = Some(s);
                }
            }
        }
    }
}

fn hex_u32(s: &str) -> Option<u32> {
    let s = s.trim();
    s.strip_prefix("0x").map_or_else(|| s.parse().ok(), |h| u32::from_str_radix(h, 16).ok())
}

impl eframe::App for App {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if self.running {
            let n = 10f64.powf(f64::from(self.speed)) as u64;
            self.advance(n);
            ctx.request_repaint();
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::Panel::top("top").show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                egui::ComboBox::from_id_salt("dev").selected_text(&self.device).show_ui(ui, |ui| {
                    for d in self.devices.clone() {
                        ui.selectable_value(&mut self.device, d.clone(), d);
                    }
                });
                ui.add(egui::TextEdit::singleline(&mut self.hex_path).hint_text("path to .hex").desired_width(320.0));
                ui.add(egui::TextEdit::singleline(&mut self.stim_path).hint_text("stimulus .toml (optional)").desired_width(180.0));
                if ui.button("Load").clicked() {
                    self.load();
                }
                ui.separator();
                let loaded = self.m.is_some();
                if ui.add_enabled(loaded && !self.running, egui::Button::new("Run")).clicked() {
                    self.running = true;
                    self.last_stop = None;
                }
                if ui.add_enabled(self.running, egui::Button::new("Pause")).clicked() {
                    self.running = false;
                }
                if ui.add_enabled(loaded && !self.running, egui::Button::new("Step")).clicked() {
                    if let Some(m) = &mut self.m {
                        m.cpu.step();
                    }
                }
                if ui.add_enabled(loaded && !self.running, egui::Button::new("Step over")).clicked() {
                    if let Some(m) = &mut self.m {
                        let w = u16::from(m.cpu.prog[m.cpu.pc as usize]) | u16::from(m.cpu.prog[m.cpu.pc as usize + 1]) << 8;
                        // CALL (two words) and RCALL step over; everything else is one step
                        let next = match w >> 8 {
                            0xEC | 0xED => Some(m.cpu.pc + 4),
                            0xD8..=0xDF => Some(m.cpu.pc + 2),
                            _ => None,
                        };
                        match next {
                            Some(t) => {
                                m.run(Some(t), 50_000_000);
                            }
                            None => {
                                m.cpu.step();
                            }
                        }
                    }
                }
                if ui.add_enabled(loaded, egui::Button::new("Reset")).clicked() {
                    self.running = false;
                    self.last_stop = None;
                    if let Some(m) = &mut self.m {
                        m.reset();
                    }
                }
                ui.add(egui::Slider::new(&mut self.speed, 0.0..=7.0).text("cycles/frame (10^x)"));
                ui.label("Fosc MHz");
                ui.add(egui::DragValue::new(&mut self.fosc_mhz).range(0.03..=64.0));
            });
            ui.label(match (&self.last_stop, self.running) {
                (_, true) => "running".to_string(),
                (Some(s), _) => format!("{} — stopped: {s:?}", self.msg),
                _ => self.msg.clone(),
            });
        });
        let Some(m) = self.m.as_mut() else {
            egui::CentralPanel::default().show(ui, |ui| {
                ui.label("Enter the path of a .hex file built by basil and press Load.");
            });
            return;
        };
        let pc = m.cpu.pc;
        if !m.cpu.p.board.is_empty() {
            let duty = m.cpu.p.board.take_window();
            self.led_duty = duty;
            egui::Panel::bottom("board").show(ui, |ui| {
                let b = &m.cpu.p.board;
                ui.horizontal_wrapped(|ui| {
                    for (l, d) in b.leds.iter().zip(&self.led_duty) {
                        ui.vertical(|ui| {
                            let (r, _) = ui.allocate_exact_size(egui::vec2(26.0, 26.0), egui::Sense::hover());
                            let v = (60.0 + 195.0 * d.clamp(0.0, 1.0)) as u8;
                            ui.painter().circle_filled(r.center(), 11.0, egui::Color32::from_rgb(v, (f32::from(v) * 0.15) as u8, (f32::from(v) * 0.1) as u8));
                            ui.small(format!("{} {}", l.name, pin_name(l.pin)));
                        });
                    }
                    for o in &b.oleds {
                        ui.vertical(|ui| {
                            let h = usize::from(o.height);
                            let (r, _) = ui.allocate_exact_size(egui::vec2(128.0 * 3.0, h as f32 * 3.0), egui::Sense::hover());
                            ui.painter().rect_filled(r, 0.0, egui::Color32::BLACK);
                            let on = egui::Color32::from_rgb(120, 200, 255);
                            for y in 0..h {
                                for x in 0..128 {
                                    if o.pixel(x, y) {
                                        let p = r.min + egui::vec2(x as f32 * 3.0, y as f32 * 3.0);
                                        ui.painter().rect_filled(egui::Rect::from_min_size(p, egui::vec2(3.0, 3.0)), 0.0, on);
                                    }
                                }
                            }
                            let st = if o.status.is_empty() { String::new() } else { format!(" - NOT WIRED: {}", o.status) };
                            ui.small(format!("OLED {} SCL {} SDA {}{st}", match o.bus { Bus::Gpio => "GPIO".to_string(), Bus::I2c(n) => format!("I2C{n}") }, pin_name(o.scl), pin_name(o.sda)));
                        });
                    }
                });
            });
        }
        egui::Panel::left("regs").default_size(230.0).show(ui, |ui| {
            ui.heading("CPU");
            let r = m.cpu.m.regs.clone();
            let mem = &m.cpu.m.mem;
            let g = |a: u32| mem[a as usize];
            let fsr = |a: u32| u32::from(g(a + 1)) << 8 | u32::from(g(a));
            let st = g(r.status);
            egui::Grid::new("regs").num_columns(2).show(ui, |ui| {
                let row = |ui: &mut egui::Ui, n: &str, v: String| {
                    ui.label(n);
                    ui.monospace(v);
                    ui.end_row();
                };
                row(ui, "PC", format!("{pc:06X}"));
                row(ui, "WREG", format!("{:02X}  ({})", g(r.wreg), g(r.wreg)));
                row(ui, "BSR", format!("{:02X}", g(r.bsr)));
                row(ui, "STATUS", format!("{st:02X}  {}", ["N", "OV", "Z", "DC", "C"].iter().zip([16u8, 8, 4, 2, 1]).map(|(n, b)| if st & b != 0 { (*n).to_string() } else { "-".repeat(n.len()) }).collect::<Vec<_>>().join(" ")));
                for i in 0..3 {
                    row(ui, &format!("FSR{i}"), format!("{:04X}", fsr(r.fsr[i])));
                }
                row(ui, "PROD", format!("{:02X}{:02X}", g(r.prodh), g(r.prodl)));
                row(ui, "TBLPTR", format!("{:06X}", u32::from(g(r.tblptr + 2)) << 16 | fsr(r.tblptr)));
                row(ui, "TABLAT", format!("{:02X}", g(r.tablat)));
                row(ui, "STKPTR", format!("{:02X}", g(r.stkptr)));
                row(ui, "Cycles", m.cpu.cycles.to_string());
                row(ui, "Instr", m.cpu.icount.to_string());
                row(ui, "Time", format!("{:.3} ms", m.cpu.cycles as f64 / (f64::from(self.fosc_mhz) * 1e6 / 4.0) * 1e3));
            });
            ui.separator();
            ui.heading("Return stack");
            let sp = usize::from(g(r.stkptr) & 0x7F).min(m.cpu.stack.len() - 1);
            for i in (1..=sp).rev().take(12) {
                ui.monospace(format!("{i:>3}  {:06X}", m.cpu.stack[i]));
            }
            if !m.cpu.m.unmodelled.is_empty() || !m.cpu.notes.is_empty() {
                ui.separator();
                ui.heading("Notes");
                for n in &m.cpu.notes {
                    ui.small(n);
                }
                if !m.cpu.m.unmodelled.is_empty() {
                    ui.small(format!("unmodelled SFRs touched: {}", m.cpu.m.unmodelled.iter().cloned().collect::<Vec<_>>().join(" ")));
                }
            }
        });
        egui::Panel::right("data").default_size(360.0).show(ui, |ui| {
            ui.horizontal(|ui| {
                for (i, t) in ["Memory", "SFRs", "Watch", "Board"].iter().enumerate() {
                    ui.selectable_value(&mut self.tab, i, *t);
                }
            });
            ui.separator();
            match self.tab {
                0 => {
                    let rows = (m.cpu.m.size / 16) as usize;
                    egui::ScrollArea::vertical().show_rows(ui, 14.0, rows, |ui, range| {
                        for row in range {
                            let a = row * 16;
                            let bytes: String = (0..16).map(|i| format!("{:02X}", m.cpu.m.mem[a + i])).collect::<Vec<_>>().join(" ");
                            let asc: String = (0..16).map(|i| { let b = m.cpu.m.mem[a + i]; if (32..127).contains(&b) { b as char } else { '.' } }).collect();
                            ui.monospace(format!("{a:04X}  {bytes}  {asc}"));
                        }
                    });
                }
                1 => {
                    ui.add(egui::TextEdit::singleline(&mut self.mem_filter).hint_text("filter"));
                    let f = self.mem_filter.to_uppercase();
                    let mut list: Vec<(u32, String)> = m.cpu.m.names.iter().enumerate().filter_map(|(a, n)| n.as_ref().map(|n| (a as u32, n.clone()))).filter(|(_, n)| f.is_empty() || n.to_uppercase().contains(&f)).collect();
                    list.sort_by(|x, y| x.1.cmp(&y.1));
                    egui::ScrollArea::vertical().show_rows(ui, 14.0, list.len(), |ui, range| {
                        for (a, n) in &list[range] {
                            let v = m.cpu.m.mem[*a as usize];
                            ui.monospace(format!("{n:<14} {a:04X}  {v:02X}  {v:08b}"));
                        }
                    });
                }
                3 => {
                    let pins = m.cpu.p.pins(&m.cpu.m);
                    let mut changed = false;
                    let mut del = None;
                    let pin_box = |ui: &mut egui::Ui, id: String, p: &mut Pin| -> bool {
                        let before = *p;
                        egui::ComboBox::from_id_salt(id).width(64.0).selected_text(pin_name(*p)).show_ui(ui, |ui| {
                            for q in &pins {
                                ui.selectable_value(p, *q, pin_name(*q));
                            }
                        });
                        *p != before
                    };
                    ui.heading("LEDs");
                    for (i, l) in m.cpu.p.board.leds.iter_mut().enumerate() {
                        ui.horizontal(|ui| {
                            ui.add(egui::TextEdit::singleline(&mut l.name).desired_width(70.0));
                            changed |= pin_box(ui, format!("led{i}"), &mut l.pin);
                            changed |= ui.checkbox(&mut l.active_high, "active high").changed();
                            if ui.small_button("x").clicked() {
                                del = Some(i);
                            }
                        });
                    }
                    if let Some(i) = del {
                        m.cpu.p.board.leds.remove(i);
                        changed = true;
                    }
                    if ui.button("Add LED").clicked() {
                        let n = m.cpu.p.board.leds.len() + 1;
                        let p = pins.first().copied().unwrap_or((b'A', 0));
                        m.cpu.p.board.leds.push(Led::new(&format!("LED{n}"), p, true));
                        changed = true;
                    }
                    ui.separator();
                    ui.heading("OLED (SSD1306, I2C)");
                    let mut remove = false;
                    for (i, o) in m.cpu.p.board.oleds.iter_mut().enumerate() {
                        egui::ComboBox::from_id_salt(format!("bus{i}")).selected_text(match o.bus { Bus::Gpio => "bit-banged GPIO".to_string(), Bus::I2c(n) => format!("hardware I2C{n}") }).show_ui(ui, |ui| {
                            for (b, t) in [(Bus::I2c(1), "hardware I2C1"), (Bus::I2c(2), "hardware I2C2"), (Bus::Gpio, "bit-banged GPIO")] {
                                if ui.selectable_value(&mut o.bus, b, t).changed() {
                                    changed = true;
                                }
                            }
                        });
                        ui.horizontal(|ui| {
                            ui.label("SCL");
                            changed |= pin_box(ui, format!("scl{i}"), &mut o.scl);
                            ui.label("SDA");
                            changed |= pin_box(ui, format!("sda{i}"), &mut o.sda);
                        });
                        ui.horizontal(|ui| {
                            ui.label("address");
                            let mut a = u32::from(o.address);
                            if ui.add(egui::DragValue::new(&mut a).range(0..=127).hexadecimal(2, false, true)).changed() {
                                o.address = a as u8;
                                changed = true;
                            }
                            for h in [64u8, 32] {
                                if ui.radio_value(&mut o.height, h, format!("{h} rows")).changed() {
                                    changed = true;
                                }
                            }
                            if ui.small_button("remove").clicked() {
                                remove = true;
                            }
                        });
                        if !o.status.is_empty() {
                            ui.colored_label(egui::Color32::from_rgb(255, 150, 80), format!("not answering: {}", o.status));
                        }
                    }
                    if remove {
                        m.cpu.p.board.oleds.clear();
                        changed = true;
                    }
                    if m.cpu.p.board.oleds.is_empty() && ui.button("Add OLED").clicked() {
                        let a = pins.iter().find(|p| **p == (b'C', 3)).copied().unwrap_or(pins[0]);
                        let d = pins.iter().find(|p| **p == (b'C', 4)).copied().unwrap_or(pins[0]);
                        m.cpu.p.board.oleds.push(Oled::new(Bus::I2c(1), a, d, 0x3C, 64));
                        changed = true;
                    }
                    if changed {
                        // A pin or address changed: re-attach the parts (the OLED starts blank, as after a power-up).
                        let b = m.cpu.p.board.clone();
                        m.cpu.p.set_board(b);
                        self.board_cfg = m.cpu.p.board.fresh();
                    }
                    ui.small("The pins must be the ones the program uses. Hardware I2C also needs the program's PPS setup (SSPxCLKPPS/SSPxDATPPS and RxyPPS) to match.");
                }
                _ => {
                    ui.horizontal(|ui| {
                        ui.add(egui::TextEdit::singleline(&mut self.watch_in).hint_text("symbol[:bytes]"));
                        if ui.button("Add").clicked() {
                            let (n, l) = self.watch_in.split_once(':').map_or((self.watch_in.as_str(), 1), |(n, l)| (n, l.parse().unwrap_or(1)));
                            self.watch.push((n.trim().to_string(), l.clamp(1, 8)));
                            self.watch_in.clear();
                        }
                    });
                    let mut del = None;
                    for (i, (n, l)) in self.watch.iter().enumerate() {
                        let addr = m.syms.get(n).copied().or_else(|| m.dev.find_sfr(n).map(|s| s.addr));
                        ui.horizontal(|ui| {
                            match addr {
                                Some(a) => {
                                    let b = m.peek(a, *l);
                                    let v: u64 = b.iter().rev().fold(0, |x, y| x << 8 | u64::from(*y));
                                    let signed = if *l < 8 { ((v << (64 - 8 * l)) as i64) >> (64 - 8 * l) } else { v as i64 };
                                    ui.monospace(format!("{n}[{l}] @{a:04X} = {v} ({signed}) 0x{v:X}"));
                                }
                                None => {
                                    ui.monospace(format!("{n}: unknown"));
                                }
                            }
                            if ui.small_button("x").clicked() {
                                del = Some(i);
                            }
                        });
                    }
                    if let Some(i) = del {
                        self.watch.remove(i);
                    }
                }
            }
        });
        egui::CentralPanel::default().show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.heading("Program");
                ui.checkbox(&mut self.follow, "follow PC");
                ui.add(egui::TextEdit::singleline(&mut self.goto_in).hint_text("goto 0x…").desired_width(80.0));
                if ui.button("Go").clicked() {
                    if let Some(a) = hex_u32(&self.goto_in) {
                        self.view_addr = a & !1;
                        self.follow = false;
                    }
                }
            });
            let prog = &m.cpu.prog;
            let word = |a: u32| -> u16 { prog.get(a as usize).map_or(0xFF, |b| u16::from(*b)) | prog.get(a as usize + 1).map_or(0xFF, |b| u16::from(*b)) << 8 };
            // Decode forward from the window start; start the window a few instructions before the PC so that
            // the listing stays aligned (it is aligned from the reset vector's instruction boundaries when
            // starting at a label or at the PC itself).
            let start = if self.follow { pc } else { self.view_addr };
            let wide = m.cpu.m.implemented[m.cpu.m.regs.fsr[0] as usize + 1] > 0x0F;
            let mut a = start;
            let lines = ((ui.available_height() / 15.0) as usize).clamp(8, 80);
            let mut bp_toggle = None;
            let mut next_view = start;
            for i in 0..lines {
                let (text, n) = disasm(a, word(a), word(a + 2), word(a + 4), wide);
                if i == 8.min(lines - 1) {
                    next_view = a;
                }
                if let Some(l) = self.labels.get(&a) {
                    ui.colored_label(egui::Color32::from_rgb(120, 170, 255), format!("{l}:"));
                }
                let has_bp = m.breakpoints.contains(&a);
                let line = format!("{} {a:06X}  {:04X}  {text}", if has_bp { "●" } else { " " }, word(a));
                let mut rt = egui::RichText::new(line).monospace();
                if a == pc {
                    rt = rt.background_color(egui::Color32::from_rgb(70, 90, 20)).color(egui::Color32::WHITE);
                } else if has_bp {
                    rt = rt.color(egui::Color32::from_rgb(255, 110, 110));
                }
                if ui.add(egui::Label::new(rt).sense(egui::Sense::click())).clicked() {
                    bp_toggle = Some(a);
                }
                a += 2 * n;
            }
            let _ = next_view;
            if let Some(b) = bp_toggle {
                if let Some(p) = m.breakpoints.iter().position(|x| *x == b) {
                    m.breakpoints.remove(p);
                } else {
                    m.breakpoints.push(b);
                }
            }
            ui.small("click an instruction to toggle a breakpoint");
        });
    }
}

fn main() -> eframe::Result {
    let mut hex = None;
    let mut device = None;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        if a == "--device" {
            device = args.next();
        } else {
            hex = Some(a);
        }
    }
    let opts = eframe::NativeOptions { viewport: egui::ViewportBuilder::default().with_inner_size([1280.0, 800.0]).with_title("basil-sim"), ..Default::default() };
    eframe::run_native("basil-sim", opts, Box::new(|_| Ok(Box::new(App::new(hex, device)))))
}
