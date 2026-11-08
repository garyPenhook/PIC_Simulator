//! `pic-sim [prog.hex] [--device 18F16Q41|16F18056]`: run, step and inspect a program on a simulated PIC18 or PIC16 (enhanced mid-range).

use eframe::egui;
use picsim::lines::LineMap;
use picsim::periph::{Board, Bus, Led, Oled, Pin, pin_name};
use picsim::machine16::Machine16;
use picsim::{Machine, Stop, disasm::disasm};
use std::collections::BTreeMap;
use std::path::PathBuf;

/// A step that runs over several frames so the window stays responsive; Pause cancels it.
enum Job {
    Line(picsim::LineStep),
    /// Run to a program address (Step over a call).
    Until(u32),
}

/// A stopwatch on the simulated instruction-cycle counter: it measures simulated time, so it is exact however fast
/// the simulation runs. Start/Stop/Lap/Clear; the counter itself is never touched.
#[derive(Default)]
struct Stopwatch {
    /// Cycle count when the elapsed time was last zeroed (adjusted on Stop/Start so a stopped interval is not counted).
    base: u64,
    /// Cycle count at which it was stopped; `None` while counting.
    stopped: Option<u64>,
    counting: bool,
    /// Elapsed cycles at each Lap press.
    laps: Vec<u64>,
}

impl Stopwatch {
    fn elapsed(&self, now: u64) -> u64 {
        if !self.counting && self.stopped.is_none() {
            return 0; // idle (never started, or cleared)
        }
        self.stopped.unwrap_or(now).saturating_sub(self.base)
    }
    fn start(&mut self, now: u64) {
        if let Some(s) = self.stopped.take() {
            self.base += now.saturating_sub(s); // resume: leave out the time it was stopped
        } else {
            self.base = now;
        }
        self.counting = true;
    }
    fn stop(&mut self, now: u64) {
        self.stopped = Some(now);
        self.counting = false;
    }
    fn clear(&mut self) {
        *self = Stopwatch::default();
    }
}

/// Stopwatch controls and readout under the CPU registers. `now` is the part's cycle count.
fn stopwatch_ui(ui: &mut egui::Ui, sw: &mut Stopwatch, now: u64, fosc_mhz: f32) {
    let us = |c: u64| c as f64 / (f64::from(fosc_mhz) / 4.0); // instruction clock is Fosc/4
    // A reset of the CPU rewinds the cycle counter: the interval no longer means anything.
    if now < sw.base || sw.stopped.is_some_and(|s| s > now) {
        sw.clear();
    }
    ui.separator();
    ui.heading("Stopwatch");
    ui.horizontal(|ui| {
        if sw.counting {
            if ui.button("Stop").clicked() {
                sw.stop(now);
            }
        } else if ui.button("Start").clicked() {
            sw.start(now);
        }
        if ui.add_enabled(sw.counting, egui::Button::new("Lap")).clicked() {
            sw.laps.push(sw.elapsed(now));
        }
        if ui.button("Clear").clicked() {
            sw.clear();
        }
    });
    let e = sw.elapsed(now);
    ui.monospace(format!("{e} cycles"));
    ui.monospace(format!("{:.3} us", us(e)));
    let mut prev = 0;
    for (i, &l) in sw.laps.iter().enumerate() {
        ui.monospace(format!("Lap {}: {:.3} us  (+{:.3})", i + 1, us(l), us(l - prev)));
        prev = l;
    }
}

struct App {
    devices_dir: PathBuf,
    devices: Vec<String>,
    device: String,
    hex_path: String,
    m: Option<Machine>,
    /// The loaded program when the part is a PIC16 (`m` and `m16` are never both set).
    m16: Option<Machine16>,
    msg: String,
    running: bool,
    speed: f32, // log10 of cycles per frame (used at "Max" run speed)
    /// Run speed as a fraction of real time (`picsim::pace::SPEEDS`; index 0 = no limit) and its clock.
    speed_idx: usize,
    pace: picsim::pace::Pace,
    fosc_mhz: f32,
    view_addr: u32,
    follow: bool,
    /// Source view: while the PC is in a library routine, show the line of the program that called it.
    stay_home: bool,
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
    pre_pins: (String, Vec<Pin>),
    led_duty: Vec<f32>,
    /// Address -> BASIC source line, from the ELF beside the hex (built with `basil -g`).
    lines: LineMap,
    /// Directory of the ELF: where a source file is looked for when its recorded path is gone.
    elf_dir: PathBuf,
    sources: std::collections::HashMap<PathBuf, Vec<String>>,
    /// Show the BASIC source instead of the disassembly.
    src_view: bool,
    /// The last source line the PC was on (generated code has none; the view stays there).
    last_src: Option<(PathBuf, u32)>,
    /// Data memory when the last Step/Run began: variables that differ are marked as changed.
    snap: Option<Vec<u8>>,
    job: Option<Job>,
    /// (time, cycles) at the last rate sample, and the measured simulated cycles per second.
    rate: (std::time::Instant, u64, f64),
    sw: Stopwatch,
}

fn list_devices(dir: &PathBuf) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| n.ends_with(".toml") && !n.ends_with(".expect.toml"))
        .map(|n| n.trim_end_matches(".toml").to_string())
        .filter(|n| n.starts_with("18F") || n.starts_with("16F"))
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
            device: device.or_else(|| devices.iter().find(|d| d.starts_with("18F")).or(devices.first()).cloned()).unwrap_or_default(),
            devices,
            hex_path: hex.clone().unwrap_or_default(),
            m: None,
            m16: None,
            msg: String::new(),
            running: false,
            speed: 6.0,
            speed_idx: 0,
            pace: picsim::pace::Pace::new(0.0, 64.0),
            fosc_mhz: 64.0,
            view_addr: 0,
            follow: true,
            stay_home: true,
            tab: 0,
            mem_filter: String::new(),
            watch: Vec::new(),
            watch_in: String::new(),
            goto_in: String::new(),
            labels: BTreeMap::new(),
            last_stop: None,
            stim_path: String::new(),
            board_cfg: Board::default(),
            pre_pins: (String::new(), Vec::new()),
            led_duty: Vec::new(),
            lines: LineMap::default(),
            elf_dir: PathBuf::new(),
            sources: std::collections::HashMap::new(),
            src_view: true,
            last_src: None,
            snap: None,
            job: None,
            rate: (std::time::Instant::now(), 0, 0.0),
            sw: Stopwatch::default(),
        };
        if hex.is_some() {
            a.load();
        }
        a
    }

    /// Opens a program given as a .hex, or as a .elf (the .hex built with it is loaded; the ELF gives the source lines).
    fn open_program(&mut self, f: &std::path::Path) {
        self.hex_path = f.display().to_string();
        self.load();
    }

    fn load(&mut self) {
        self.running = false;
        self.sw.clear();
        self.job = None; // a step over from the previous program must not run on the new one
        self.last_stop = None;
        // The part the ELF was built for wins over the device box (the wrong part has the wrong SFRs and pins).
        let mut dev_note = String::new();
        if let Some(e) = picsim::debuginfo::elf_for(std::path::Path::new(self.hex_path.trim()))
            && let Ok(b) = std::fs::read(&e)
            && let Some(d) = picsim::debuginfo::device_of(&b)
            && let Some(x) = self.devices.iter().find(|x| x.eq_ignore_ascii_case(&d))
            && *x != self.device
        {
            dev_note = format!(" - device set to {x} from the ELF (was {})", self.device);
            self.device = x.clone();
        }
        self.m16 = None;
        let is16 = devdesc::Device::load(&self.devices_dir.join(format!("{}.toml", self.device))).is_ok_and(|d| d.device.family == "pic16e");
        if is16 {
            self.load16(dev_note);
            return;
        }
        let res = (|| -> Result<(Machine, String), String> {
            let dev = devdesc::Device::load(&self.devices_dir.join(format!("{}.toml", self.device))).map_err(|e| e.to_string())?;
            let mut m = Machine::new(dev)?;
            let p = picsim::lines::program_hex(std::path::Path::new(self.hex_path.trim()))?; // a .elf loads its .hex
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
            // Routines, variables and source lines from the ELF: <stem>.elf beside the hex, or one folder up (basil writes the hex into dist/).
            let note = match picsim::debuginfo::elf_for(std::path::Path::new(self.hex_path.trim())) {
                Some(e) => match m.load_elf(&e) {
                    Ok(()) => {
                        self.elf_dir = e.parent().map(PathBuf::from).unwrap_or_default();
                        if m.debug.lines.source_files().is_empty() { " - ELF has no source lines (build with basil -g or xc8-cc -g)".to_string() } else { String::new() }
                    }
                    Err(er) => format!(" - {er}"),
                },
                None => " - no .elf beside the hex: no source view or variables".to_string(),
            };
            Ok((m, format!("{dev_note}{note}")))
        })();
        match res {
            Ok((m, note)) => {
                self.labels.clear();
                for (n, a) in &m.syms {
                    if m.cpu.prog.get(*a as usize).is_some_and(|b| *b != 0xFF) && !n.starts_with("T_") && !m.debug.vars.iter().any(|v| v.scope.is_none() && v.name == *n) {
                        self.labels.entry(*a).or_insert_with(|| n.clone());
                    }
                }
                let mut m = m;
                if m.cpu.p.board.is_empty() {
                    m.cpu.p.set_board(self.board_cfg.fresh()); // keep the parts wired in an earlier session
                } else {
                    self.board_cfg = m.cpu.p.board.fresh(); // the stimulus file wired them
                }
                self.sources.clear();
                self.last_src = None;
                self.snap = None;
                // BASIC lines only: an ELF built without -g has just the assembly's lines.
                self.lines = if m.debug.lines.source_files().is_empty() { LineMap::default() } else { m.debug.lines.clone() };
                if !m.debug.vars.is_empty() {
                    self.tab = 4; // the Variables tab
                }
                self.msg = format!("loaded {} ({} symbols, {} variables){note}", self.hex_path, m.syms.len(), m.debug.vars.len());
                self.m = Some(m);
            }
            Err(e) => {
                self.msg = e;
                self.m = None;
            }
        }
    }

    /// Loads a program for a PIC16 part: the hex, the assembly listing's labels, the stimulus file and the ELF beside the hex
    /// (routines, variables and source lines; its addresses are words, like the PC).
    fn load16(&mut self, dev_note: String) {
        let res = (|| -> Result<(Machine16, BTreeMap<u32, String>, String), String> {
            let dev = devdesc::Device::load(&self.devices_dir.join(format!("{}.toml", self.device))).map_err(|e| e.to_string())?;
            let mut m = Machine16::new(dev)?;
            let p = picsim::lines::program_hex(std::path::Path::new(self.hex_path.trim()))?;
            m.load_hex(&std::fs::read_to_string(&p).map_err(|e| format!("{}: {e}", p.display()))?)?;
            let mut labels = BTreeMap::new();
            if let Ok(t) = std::fs::read_to_string(p.with_extension("S")) {
                m.load_symbols_from_asm(&t);
                // Only the `;SYM` lines are code labels (an `equ` is a variable's data address).
                for l in t.lines().filter_map(|l| l.strip_prefix(";SYM ")) {
                    let mut it = l.split_whitespace();
                    if let (Some(n), Some(a)) = (it.next(), it.next().and_then(|a| a.strip_prefix("0x")).and_then(|h| u32::from_str_radix(h, 16).ok())) {
                        labels.entry(a).or_insert_with(|| n.to_string());
                    }
                }
            }
            if !self.stim_path.trim().is_empty() {
                let f = PathBuf::from(self.stim_path.trim());
                m.load_stimulus(&std::fs::read_to_string(&f).map_err(|e| format!("{}: {e}", f.display()))?).map_err(|e| format!("{}: {e}", f.display()))?;
            }
            let note = match picsim::debuginfo::elf_for(std::path::Path::new(self.hex_path.trim())) {
                Some(e) => match m.load_elf(&e) {
                    Ok(()) => {
                        self.elf_dir = e.parent().map(PathBuf::from).unwrap_or_default();
                        if m.debug.lines.source_files().is_empty() { " - ELF has no source lines (build with basil -g or xc8-cc -g)".to_string() } else { String::new() }
                    }
                    Err(er) => format!(" - {er}"),
                },
                None => " - no .elf beside the hex: no source view or variables".to_string(),
            };
            Ok((m, labels, note))
        })();
        self.m = None;
        self.sources.clear();
        self.last_src = None;
        self.snap = None;
        self.lines = LineMap::default();
        match res {
            Ok((mut m, labels, note)) => {
                let mut labels = labels;
                for f in &m.debug.funcs {
                    labels.entry(f.low).or_insert_with(|| f.name.clone());
                }
                if m.cpu.board.is_empty() {
                    m.cpu.set_board(self.board_cfg.fresh()); // keep the parts wired in an earlier session
                } else {
                    self.board_cfg = m.cpu.board.fresh(); // the stimulus file wired them
                }
                self.lines = if m.debug.lines.source_files().is_empty() { LineMap::default() } else { m.debug.lines.clone() };
                self.src_view = !self.lines.is_empty();
                self.tab = if m.debug.vars.is_empty() { 0 } else { 4 }; // the Variables tab
                self.msg = format!("loaded {} ({} labels, {} variables){dev_note}{note}", self.hex_path, labels.len(), m.debug.vars.len());
                self.labels = labels;
                self.fosc_mhz = self.fosc_mhz.min(32.0);
                self.m16 = Some(m);
            }
            Err(e) => {
                self.msg = format!("{e}{dev_note}");
                self.labels.clear();
            }
        }
    }

    /// Instruction cycles executed so far by whichever part is loaded.
    fn cycles(&self) -> u64 {
        self.m.as_ref().map(|m| m.cpu.cycles).or_else(|| self.m16.as_ref().map(|m| m.cpu.cycles)).unwrap_or(0)
    }

    fn snapshot(&mut self) {
        self.snap = self.m.as_ref().map(|m| m.cpu.m.mem.clone()).or_else(|| self.m16.as_ref().map(|m| m.cpu.mem.clone()));
    }

    /// Steps to the next BASIC line. `over` runs calls (and interrupts) to completion; `out` runs until the current
    /// routine returns. Stops early at a breakpoint, at `__halt`, or after a million instructions.
    fn step_line(&mut self, over: bool, out: bool) {
        self.snapshot();
        if let Some(m) = &self.m {
            self.job = Some(Job::Line(m.begin_line_step(over, out)));
        } else if let Some(m) = &self.m16 {
            self.job = Some(Job::Line(m.begin_line_step(over, out)));
        }
    }

    fn advance(&mut self, cycles: u64) {
        if let Some(m) = &mut self.m16 {
            let halt = m.syms.get("__halt").copied();
            match m.run(halt, cycles) {
                Stop::Cycles => {}
                s => {
                    self.running = false;
                    self.last_stop = Some(s);
                }
            }
        }
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
        // Work in slices of about 30 ms, so a long step or a fast Run never freezes the window.
        let t0 = std::time::Instant::now();
        let slice = std::time::Duration::from_millis(30);
        if let Some(job) = self.job.take() {
            let mut keep = Some(job);
            if let Some(m) = self.m16.as_mut() {
                while let Some(j) = keep.as_ref() {
                    // None: not finished. Some(None): arrived. Some(Some(stop)): a breakpoint or the halt came first.
                    let finished: Option<Option<Stop>> = match j {
                        Job::Line(s) => m.line_step(s, 20_000),
                        Job::Until(t) => match m.run(Some(*t), 20_000) {
                            Stop::Cycles => None,
                            Stop::Halt => Some(None), // the target address
                            s => Some(Some(s)),
                        },
                    };
                    if let Some(r) = finished {
                        self.last_stop = r;
                        keep = None;
                    } else if t0.elapsed() > slice {
                        break;
                    }
                }
            } else if let Some(m) = self.m.as_mut() {
                while let Some(j) = keep.as_ref() {
                    // None: not finished. Some(None): arrived. Some(Some(stop)): a breakpoint or the halt came first.
                    let finished: Option<Option<Stop>> = match j {
                        Job::Line(s) => m.line_step(s, 20_000),
                        Job::Until(t) => match m.run(Some(*t), 20_000) {
                            Stop::Cycles => None,
                            Stop::Halt => Some(None), // the target address
                            s => Some(Some(s)),
                        },
                    };
                    if let Some(r) = finished {
                        self.last_stop = r;
                        keep = None;
                    } else if t0.elapsed() > slice {
                        break;
                    }
                }
            } else {
                keep = None;
            }
            if keep.is_some() {
                ctx.request_repaint();
            }
            self.job = keep;
        }
        if self.running {
            // A changed speed or Fosc re-bases the clock (it would otherwise jump); the Run button does the same.
            let ratio = picsim::pace::SPEEDS[self.speed_idx].1;
            if ratio != self.pace.ratio || f64::from(self.fosc_mhz) != self.pace.fosc_mhz {
                self.pace.ratio = ratio;
                self.pace.fosc_mhz = f64::from(self.fosc_mhz);
                let c = self.cycles();
                self.pace.restart(c);
            }
            let n = if self.pace.is_max() {
                10f64.powf(f64::from(self.speed)) as u64
            } else {
                let c = self.cycles();
                self.pace.due(c)
            };
            let mut left = n;
            while left > 0 && self.running {
                let chunk = left.min(20_000);
                self.advance(chunk);
                left -= chunk;
                if t0.elapsed() > slice {
                    break;
                }
            }
            ctx.request_repaint();
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        if self.job.is_some() {
            ui.ctx().request_repaint(); // keep the slices coming
        }
        // A file dropped on the window: .hex loads as the program, .toml is the stimulus.
        let dropped: Vec<PathBuf> = ui.ctx().input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).collect());
        for f in dropped {
            match f.extension().and_then(|e| e.to_str()).map(str::to_lowercase).as_deref() {
                Some("hex" | "elf") => {
                    if !self.running {
                        self.open_program(&f);
                    }
                }
                Some("toml") => self.stim_path = f.display().to_string(),
                _ => {}
            }
        }
        egui::Panel::top("top").show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                egui::ComboBox::from_id_salt("dev").selected_text(&self.device).show_ui(ui, |ui| {
                    for d in self.devices.clone() {
                        ui.selectable_value(&mut self.device, d.clone(), d);
                    }
                });
                ui.add(egui::TextEdit::singleline(&mut self.hex_path).hint_text("path to .hex or .elf").desired_width(320.0));
                ui.add(egui::TextEdit::singleline(&mut self.stim_path).hint_text("stimulus .toml (optional)").desired_width(180.0))
                    .on_hover_text("Optional. A stimulus file you write to feed the program serial input, ADC values, pin levels, LEDs or an OLED. Leave empty to run without one.");
                // Start the dialogs in the program's folder, not wherever the desktop last was.
                let dir_of = |p: &str| std::path::Path::new(p.trim()).parent().filter(|d| d.is_dir()).map(PathBuf::from);
                let prog_dir = dir_of(&self.hex_path);
                if ui.button("Browse hex/elf…").clicked() {
                    let mut d = rfd::FileDialog::new().add_filter("hex or ELF", &["hex", "elf"]);
                    if let Some(dir) = &prog_dir {
                        d = d.set_directory(dir);
                    }
                    if let Some(f) = d.pick_file() {
                        self.open_program(&f);
                    }
                }
                if ui.button("Stim…").on_hover_text("Choose a stimulus .toml. The dialog opens in the stimulus file's folder, else the program's folder, else the examples in tests/sim.").clicked() {
                    let examples = self.devices_dir.join("../tests/sim");
                    let mut d = rfd::FileDialog::new().add_filter("stimulus", &["toml"]);
                    if let Some(dir) = dir_of(&self.stim_path).or(prog_dir).or_else(|| examples.is_dir().then_some(examples)) {
                        d = d.set_directory(dir);
                    }
                    if let Some(f) = d.pick_file() {
                        self.stim_path = f.display().to_string();
                    }
                }
                if !self.stim_path.is_empty() && ui.small_button("x").on_hover_text("Run without a stimulus file").clicked() {
                    self.stim_path.clear();
                }
                if ui.button("Load").clicked() {
                    self.load();
                }
                ui.separator();
                let loaded = self.m.is_some() || self.m16.is_some();
                let idle = !self.running && self.job.is_none();
                if ui.add_enabled(loaded && idle, egui::Button::new("Run")).clicked() {
                    self.snapshot();
                    self.rate = (std::time::Instant::now(), self.cycles(), 0.0);
                    self.pace.ratio = picsim::pace::SPEEDS[self.speed_idx].1;
                    self.pace.fosc_mhz = f64::from(self.fosc_mhz);
                    let c = self.cycles();
                    self.pace.restart(c);
                    self.running = true;
                    self.last_stop = None;
                }
                if ui.add_enabled(!idle, egui::Button::new("Pause")).clicked() {
                    self.running = false;
                    self.job = None;
                }
                if ui.add_enabled(loaded && idle, egui::Button::new("Step")).clicked() {
                    self.snapshot();
                    if let Some(m) = &mut self.m {
                        m.cpu.step();
                    }
                    if let Some(m) = &mut self.m16 {
                        m.cpu.step();
                    }
                }
                if ui.add_enabled(loaded && idle, egui::Button::new("Step over")).clicked() {
                    self.snapshot();
                    if let Some(m) = &mut self.m16 {
                        // CALL and CALLW step over (one word each); everything else is one step.
                        let w = m.cpu.fetch(m.cpu.pc);
                        if w >> 11 == 4 || w == 0x000A {
                            self.job = Some(Job::Until((m.cpu.pc + 1) % (m.cpu.prog.len() as u32 / 2)));
                        } else {
                            m.cpu.step();
                        }
                    }
                    if let Some(m) = &mut self.m {
                        let w = u16::from(m.cpu.prog[m.cpu.pc as usize]) | u16::from(m.cpu.prog[m.cpu.pc as usize + 1]) << 8;
                        // CALL (two words) and RCALL step over; everything else is one step
                        let next = match w >> 8 {
                            0xEC | 0xED => Some(m.cpu.pc + 4),
                            0xD8..=0xDF => Some(m.cpu.pc + 2),
                            _ => None,
                        };
                        match next {
                            Some(t) => self.job = Some(Job::Until(t)),
                            None => {
                                m.cpu.step();
                            }
                        }
                    }
                }
                let has_src = loaded && !self.lines.is_empty();
                if ui.add_enabled(has_src && idle, egui::Button::new("Line")).on_hover_text("step to the next BASIC line, into calls").clicked() {
                    self.step_line(false, false);
                }
                if ui.add_enabled(has_src && idle, egui::Button::new("Line over")).on_hover_text("step to the next BASIC line, over calls").clicked() {
                    self.step_line(true, false);
                }
                if ui.add_enabled(has_src && idle, egui::Button::new("Out")).on_hover_text("run until this routine returns").clicked() {
                    self.step_line(false, true);
                }
                if ui.add_enabled(loaded, egui::Button::new("Reset")).clicked() {
                    self.running = false;
                    self.last_stop = None;
                    self.job = None;
                    self.snap = None;
                    self.sw.clear();
                    if let Some(m) = &mut self.m {
                        m.reset();
                    }
                    if let Some(m) = &mut self.m16
                        && let Err(e) = m.reset()
                    {
                        self.msg = e;
                    }
                }
                egui::ComboBox::from_id_salt("speed").selected_text(format!("Speed: {}", picsim::pace::SPEEDS[self.speed_idx].0)).show_ui(ui, |ui| {
                    for (i, (name, _)) in picsim::pace::SPEEDS.iter().enumerate() {
                        ui.selectable_value(&mut self.speed_idx, i, *name);
                    }
                });
                if self.speed_idx == 0 {
                    ui.add(egui::Slider::new(&mut self.speed, 0.0..=7.0).text("cycles/frame (10^x)"));
                }
                ui.label("Fosc MHz");
                ui.add(egui::DragValue::new(&mut self.fosc_mhz).range(0.03..=64.0));
            });
            // Simulated cycles per second while running, against what the part does at the chosen Fosc.
            if self.running {
                let cycles = self.cycles();
                let dt = self.rate.0.elapsed().as_secs_f64();
                if dt >= 0.5 {
                    self.rate = (std::time::Instant::now(), cycles, cycles.saturating_sub(self.rate.1) as f64 / dt);
                }
            }
            let real = f64::from(self.fosc_mhz) * 1e6 / 4.0;
            ui.label(match (&self.last_stop, self.running) {
                (_, true) => format!("running at {:.1}% of real time ({:.2} Mcycles/s); a wait on a UART or timer flag shows as one line for a long time", 100.0 * self.rate.2 / real, self.rate.2 / 1e6),
                _ if self.job.is_some() => "stepping... (Pause cancels)".to_string(),
                (Some(s), _) => format!("{} — stopped: {s:?}", self.msg),
                _ => self.msg.clone(),
            });
        });
        if self.m16.is_some() {
            self.ui16(ui);
            return;
        }
        let Some(m) = self.m.as_mut() else {
            // No program yet: still let the LEDs and OLED be wired; they are kept for the next Load.
            if self.pre_pins.0 != self.device {
                let pins = devdesc::Device::load(&self.devices_dir.join(format!("{}.toml", self.device)))
                    .ok()
                    .and_then(|d| if d.device.family == "pic16e" { Machine16::new(d).ok().map(|m| m.cpu.pins()) } else { Machine::new(d).ok().map(|m| m.cpu.p.pins(&m.cpu.m)) })
                    .unwrap_or_default();
                self.pre_pins = (self.device.clone(), pins);
            }
            egui::Panel::right("data").default_size(360.0).show(ui, |ui| {
                ui.heading("Board");
                if self.pre_pins.1.is_empty() {
                    ui.label("Pick a device to wire LEDs and an OLED.");
                } else if board_ui(ui, &mut self.board_cfg, &self.pre_pins.1) {
                    self.board_cfg = self.board_cfg.fresh();
                }
            });
            egui::CentralPanel::default().show(ui, |ui| {
                ui.label("Enter the path of a .hex file built by basil and press Load. LEDs and an OLED can be wired in the Board panel first.");
            });
            return;
        };
        let pc = m.cpu.pc;
        m.cpu.p.board.fosc_mhz = f64::from(self.fosc_mhz);
        if !m.cpu.p.board.is_empty() {
            let duty = m.cpu.p.board.take_window();
            self.led_duty = duty;
            egui::Panel::bottom("board").show(ui, |ui| board_strip(ui, &m.cpu.p.board, &self.led_duty));
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
            stopwatch_ui(ui, &mut self.sw, m.cpu.cycles, self.fosc_mhz);
            ui.separator();
            ui.heading(if m.debug.funcs.is_empty() { "Return stack" } else { "Call stack" });
            let sp = usize::from(g(r.stkptr) & 0x7F).min(m.cpu.stack.len() - 1);
            let dbg = &m.debug;
            // Routine and BASIC line of a program address.
            let loc = |a: u32| -> String {
                let f = dbg.func_at(a).map_or("?", |i| dbg.funcs[i].name.as_str());
                match dbg.lines.lookup(a) {
                    Some((p, l)) => format!("{f}  {}:{l}", p.file_name().unwrap_or_default().to_string_lossy()),
                    None => f.to_string(),
                }
            };
            if !dbg.funcs.is_empty() {
                ui.monospace(format!("> {}", loc(pc)));
            }
            for i in (1..=sp).rev().take(12) {
                // The return address follows the CALL; look at the instruction before it.
                let ret = m.cpu.stack[i];
                if dbg.funcs.is_empty() {
                    ui.monospace(format!("{i:>3}  {ret:06X}"));
                } else {
                    ui.monospace(format!("{i:>3}  {}", loc(ret.saturating_sub(2))));
                }
            }
            if !m.cpu.m.unmodelled.is_empty() || !m.cpu.notes.is_empty() {
                ui.separator();
                ui.heading("Notes");
                for n in &m.cpu.notes {
                    ui.label(n);
                }
                if !m.cpu.m.unmodelled.is_empty() {
                    ui.label(format!("unmodelled SFRs touched: {}", m.cpu.m.unmodelled.iter().cloned().collect::<Vec<_>>().join(" ")));
                }
            }
        });
        egui::Panel::right("data").default_size(360.0).show(ui, |ui| {
            ui.horizontal(|ui| {
                for (i, t) in ["Memory", "SFRs", "Watch", "Board", "Variables"].iter().enumerate() {
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
                            let bytes: String = (0..16).map(|i| format!("{:02X}", m.cpu.peek((a + i) as u32))).collect::<Vec<_>>().join(" ");
                            let asc: String = (0..16).map(|i| { let b = m.cpu.peek((a + i) as u32); if (32..127).contains(&b) { b as char } else { '.' } }).collect();
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
                            let v = m.cpu.peek(*a);
                            ui.monospace(format!("{n:<14} {a:04X}  {v:02X}  {v:08b}"));
                        }
                    });
                }
                3 => {
                    let pins = m.cpu.p.pins(&m.cpu.m);
                    if board_ui(ui, &mut m.cpu.p.board, &pins) {
                        // A pin or address changed: re-attach the parts (the OLED starts blank, as after a power-up).
                        let b = m.cpu.p.board.clone();
                        m.cpu.p.set_board(b);
                        self.board_cfg = m.cpu.p.board.fresh();
                    }
                }
                4 => variables_ui(ui, &m.debug, pc, &(1..=usize::from(m.cpu.m.mem[m.cpu.m.regs.stkptr as usize] & 0x7F).min(m.cpu.stack.len() - 1)).rev().map(|i| m.cpu.stack[i].saturating_sub(2)).collect::<Vec<_>>(), &m.cpu.m.mem, &m.cpu.prog, self.snap.as_ref()),
                _ => {
                    let (syms, vars, dev, debug) = (&m.syms, &m.debug.vars, &m.dev, &m.debug);
                    watch_ui(ui, &mut self.watch, &mut self.watch_in, |n| syms.get(n).copied().map(|a| (a, false)).or_else(|| vars.iter().find(|v| v.scope.is_none() && !v.rom && v.name == *n).map(|v| (v.addr, false))).or_else(|| debug.path_ref(n).map(|p| (p.0, p.2))).or_else(|| dev.find_sfr(n).map(|s| (s.addr, false))), |a, l, rom| if rom { debug.rom_bytes(&m.cpu.prog, a, l) } else { m.peek(a, l) });
                }
            }
        });
        egui::CentralPanel::default().show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.heading("Program");
                if !self.lines.is_empty() {
                    ui.selectable_value(&mut self.src_view, true, "Source");
                    ui.selectable_value(&mut self.src_view, false, "Disassembly");
                }
                ui.checkbox(&mut self.follow, "follow PC");
                if self.src_view && !self.lines.is_empty() {
                    ui.checkbox(&mut self.stay_home, "stay in my code").on_hover_text("while the PC is in a library routine (DelaySeconds ...), show the line of your program that called it");
                }
                ui.add(egui::TextEdit::singleline(&mut self.goto_in).hint_text("goto 0x…").desired_width(80.0));
                if ui.button("Go").clicked() {
                    if let Some(a) = hex_u32(&self.goto_in) {
                        self.view_addr = a & !1;
                        self.follow = false;
                    }
                }
            });
            if self.lines.is_empty() {
                ui.colored_label(egui::Color32::from_rgb(255, 150, 80), "No source line information in this build. Build with basil -g or xc8-cc -g (VS Code: Build Debug, Ctrl+F7; F7 has no line information) and load dist/<name>.hex or .elf: then Source, Variables, Line stepping and the call stack work.");
            }
            if self.src_view && !self.lines.is_empty() {
                let home = std::path::Path::new(self.hex_path.trim()).file_stem().map(|s| s.to_string_lossy().trim_end_matches("_debug").to_ascii_lowercase());
                let sp = usize::from(m.cpu.m.mem[m.cpu.m.regs.stkptr as usize] & 0x7F).min(m.cpu.stack.len() - 1);
                let callers: Vec<u32> = (1..=sp).rev().map(|i| m.cpu.stack[i].saturating_sub(2)).collect(); // the CALL is the instruction before the return address
                source_pane(ui, &self.lines, &self.elf_dir, &mut self.sources, &mut self.last_src, &mut m.breakpoints, &callers, pc, self.follow, if self.stay_home { home } else { None });
                return;
            }
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
            ui.label("click an instruction to toggle a breakpoint");
        });
    }
}

/// The routines that get a "caller" section: those holding a pending CALL (innermost first), each once, without the current
/// routine `cur` and without routines that have no parameters or locals.
fn caller_frames(debug: &picsim::debuginfo::DebugInfo, cur: Option<usize>, callers: &[u32]) -> Vec<usize> {
    let mut frames: Vec<usize> = Vec::new();
    for &c in callers {
        if let Some(i) = debug.func_at(c).filter(|&i| Some(i) != cur && !frames.contains(&i) && debug.vars.iter().any(|v| v.scope == Some(i))) {
            frames.push(i);
        }
    }
    frames
}

/// The Variables tab: the globals and the parameters and locals of the routine the PC is in, with their values in `mem`;
/// those that differ from `snap` (data memory when the last Step or Run began) are marked. `callers` are the addresses of the
/// pending CALL instructions, innermost first; each routine they lie in gets its own section (once, even when it recurses,
/// because its locals have one fixed address) below the current routine's.
fn variables_ui(ui: &mut egui::Ui, debug: &picsim::debuginfo::DebugInfo, pc: u32, callers: &[u32], mem: &[u8], prog: &[u8], snap: Option<&Vec<u8>>) {
    if debug.vars.is_empty() {
        ui.label("No variable information. Build with basil -g or xc8-cc -g and keep the .elf beside the .hex.");
        return;
    }
    let f = debug.func_at(pc);
    ui.label(match f {
        Some(i) => format!("in {}", debug.funcs[i].name),
        None => "in start-up code (globals only)".to_string(),
    });
    let rom = debug.rom_image(prog);
    let frames = caller_frames(debug, f, callers);
    egui::ScrollArea::vertical().show(ui, |ui| {
        let row = |ui: &mut egui::Ui, v: &picsim::debuginfo::Var| {
            let a = v.addr as usize;
            let changed = !v.rom && snap.is_some_and(|s| s.get(a..a + v.bytes as usize) != mem.get(a..a + v.bytes as usize));
            let col = if changed { egui::Color32::from_rgb(255, 170, 60) } else { ui.visuals().text_color() };
            ui.colored_label(col, format!("{}{}", v.name, if v.param { " (arg)" } else { "" }));
            ui.weak(format!("{} @{:03X}", v.ty, v.addr));
            ui.add(egui::Label::new(egui::RichText::new(v.show_with(mem, &rom, debug)).monospace().color(col)).wrap());
            ui.end_row();
        };
        egui::Grid::new("vars").num_columns(3).striped(true).show(ui, |ui| {
            for v in debug.visible(f) {
                row(ui, v);
            }
        });
        for (n, &i) in frames.iter().enumerate() {
            ui.separator();
            ui.label(format!("caller: {}", debug.funcs[i].name));
            egui::Grid::new(format!("vars_caller{n}")).num_columns(3).striped(true).show(ui, |ui| {
                for v in debug.vars.iter().filter(|v| v.scope == Some(i)) {
                    row(ui, v);
                }
            });
        }
    });
    ui.label("orange: changed since the last Step or Run");
}

/// The Watch tab: symbols (or SFR names) with a byte count, shown as unsigned, signed and hex. `addr` resolves a name,
/// `peek` reads bytes without side effects.
fn watch_ui(ui: &mut egui::Ui, watch: &mut Vec<(String, usize)>, input: &mut String, addr: impl Fn(&str) -> Option<(u32, bool)>, peek: impl Fn(u32, usize, bool) -> Vec<u8>) {
    ui.horizontal(|ui| {
        ui.add(egui::TextEdit::singleline(input).hint_text("symbol[:bytes]"));
        if ui.button("Add").clicked() {
            let (n, l) = input.split_once(':').map_or((input.as_str(), 1), |(n, l)| (n, l.parse().unwrap_or(1)));
            watch.push((n.trim().to_string(), l.clamp(1, 8)));
            input.clear();
        }
    });
    let mut del = None;
    for (i, (n, l)) in watch.iter().enumerate() {
        ui.horizontal(|ui| {
            match addr(n) {
                Some((a, rom)) => {
                    let v: u64 = peek(a, *l, rom).iter().rev().fold(0, |x, y| x << 8 | u64::from(*y));
                    let signed = if *l < 8 { ((v << (64 - 8 * l)) as i64) >> (64 - 8 * l) } else { v as i64 };
                    ui.monospace(format!("{n}[{l}] @{}{a:04X} = {v} ({signed}) 0x{v:X}", if rom { "prog " } else { "" }));
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
        watch.remove(i);
    }
}

impl App {
    /// The window body for a PIC16 part: core registers and return stack, Memory / SFRs / Watch, and the disassembly.
    fn ui16(&mut self, ui: &mut egui::Ui) {
        let Some(m) = self.m16.as_mut() else { return };
        let pc = m.cpu.pc;
        m.cpu.board.fosc_mhz = f64::from(self.fosc_mhz);
        if !m.cpu.board.is_empty() {
            self.led_duty = m.cpu.board.take_window();
            egui::Panel::bottom("board").show(ui, |ui| board_strip(ui, &m.cpu.board, &self.led_duty));
        }
        egui::Panel::left("regs").default_size(230.0).show(ui, |ui| {
            ui.heading("CPU");
            let g = |a: usize| m.cpu.mem[a];
            // Core registers sit at the same addresses on every enhanced mid-range part: STATUS 03h, FSR0 04h/05h, FSR1 06h/07h, BSR 08h, WREG 09h, PCLATH 0Ah, INTCON 0Bh.
            let st = g(3);
            egui::Grid::new("regs16").num_columns(2).show(ui, |ui| {
                let row = |ui: &mut egui::Ui, n: &str, v: String| {
                    ui.label(n);
                    ui.monospace(v);
                    ui.end_row();
                };
                row(ui, "PC", format!("{pc:04X}"));
                row(ui, "WREG", format!("{:02X}  ({})", g(9), g(9)));
                row(ui, "BSR", format!("{:02X}", g(8)));
                row(ui, "STATUS", format!("{st:02X}  {}", [("Z", picsim::pic16::Z), ("DC", picsim::pic16::DC), ("C", picsim::pic16::C)].iter().map(|(n, b)| if st & b != 0 { (*n).to_string() } else { "-".repeat(n.len()) }).collect::<Vec<_>>().join(" ")));
                row(ui, "PCLATH", format!("{:02X}", g(10)));
                row(ui, "INTCON", format!("{:02X}", g(11)));
                row(ui, "FSR0", format!("{:02X}{:02X}", g(5), g(4)));
                row(ui, "FSR1", format!("{:02X}{:02X}", g(7), g(6)));
                row(ui, "Cycles", m.cpu.cycles.to_string());
                row(ui, "Instr", m.cpu.icount.to_string());
                row(ui, "Time", format!("{:.3} ms", m.cpu.cycles as f64 / (f64::from(self.fosc_mhz) * 1e6 / 4.0) * 1e3));
            });
            stopwatch_ui(ui, &mut self.sw, m.cpu.cycles, self.fosc_mhz);
            ui.separator();
            let dbg = &m.debug;
            ui.heading(if dbg.funcs.is_empty() { "Return stack" } else { "Call stack" });
            // Routine and BASIC line of a program (word) address.
            let loc = |a: u32| -> String {
                let f = dbg.func_at(a).map_or("?", |i| dbg.funcs[i].name.as_str());
                match dbg.lines.lookup(a) {
                    Some((p, l)) => format!("{f}  {}:{l}", p.file_name().unwrap_or_default().to_string_lossy()),
                    None => f.to_string(),
                }
            };
            if !dbg.funcs.is_empty() {
                ui.monospace(format!("> {}", loc(pc)));
            }
            let depth = m.cpu.depth() as usize;
            for i in (0..depth.min(16)).rev() { // STKPTR keeps counting after an overflow with STVREN = 0, but only 16 entries exist
                let ret = m.cpu.stack_entry(i);
                if dbg.funcs.is_empty() {
                    // The entry is the address after the CALL (or the interrupted instruction): name the label at or before it.
                    let near = self.labels.range(..=ret.saturating_sub(1)).next_back().map_or(String::new(), |(a, n)| format!("  {n}+{}", ret.saturating_sub(1) - a));
                    ui.monospace(format!("{:>3}  {ret:04X}{near}", i + 1));
                } else {
                    ui.monospace(format!("{:>3}  {}", i + 1, loc(ret.saturating_sub(1)))); // the CALL is the word before the return address
                }
            }
            if !m.cpu.unmodelled.is_empty() || !m.cpu.notes.is_empty() {
                ui.separator();
                ui.heading("Notes");
                for n in &m.cpu.notes {
                    ui.label(n);
                }
                if !m.cpu.unmodelled.is_empty() {
                    ui.label(format!("unmodelled SFRs touched: {}", m.cpu.unmodelled.iter().cloned().collect::<Vec<_>>().join(" ")));
                }
            }
        });
        egui::Panel::right("data").default_size(360.0).show(ui, |ui| {
            ui.horizontal(|ui| {
                for (i, t) in ["Memory", "SFRs", "Watch", "Board", "Variables"].iter().enumerate() {
                    ui.selectable_value(&mut self.tab, i, *t);
                }
            });
            ui.separator();
            match self.tab {
                0 => {
                    // Up to the last address that has any bit; an address with none (unimplemented, or an alias of another bank's core registers) shows --.
                    let top = m.cpu.implemented.iter().rposition(|b| *b != 0).map_or(0, |i| i + 1);
                    egui::ScrollArea::vertical().show_rows(ui, 14.0, top.div_ceil(16), |ui, range| {
                        for row in range {
                            let a = row * 16;
                            let has = |i: usize| m.cpu.implemented.get(a + i).is_some_and(|b| *b != 0);
                            let bytes: String = (0..16).map(|i| if has(i) { format!("{:02X}", m.cpu.peek((a + i) as u32)) } else { "--".to_string() }).collect::<Vec<_>>().join(" ");
                            let asc: String = (0..16).map(|i| { let b = m.cpu.peek((a + i) as u32); if has(i) && (32..127).contains(&b) { b as char } else { '.' } }).collect();
                            ui.monospace(format!("{a:04X}  {bytes}  {asc}"));
                        }
                    });
                }
                1 => {
                    ui.add(egui::TextEdit::singleline(&mut self.mem_filter).hint_text("filter"));
                    let f = self.mem_filter.to_uppercase();
                    let mut list: Vec<(&str, u32)> = m.dev.sfr.iter().filter(|s| f.is_empty() || s.name.to_uppercase().contains(&f)).map(|s| (s.name.as_str(), s.addr)).collect();
                    list.sort();
                    egui::ScrollArea::vertical().show_rows(ui, 14.0, list.len(), |ui, range| {
                        for (n, a) in &list[range] {
                            let v = m.cpu.peek(*a);
                            ui.monospace(format!("{n:<14} {a:04X}  {v:02X}  {v:08b}"));
                        }
                    });
                }
                3 => {
                    let pins = m.cpu.pins();
                    if board_ui(ui, &mut m.cpu.board, &pins) {
                        // A pin or address changed: re-attach the parts (the OLED starts blank, as after a power-up).
                        let b = m.cpu.board.clone();
                        m.cpu.set_board(b);
                        self.board_cfg = m.cpu.board.fresh();
                    }
                }
                4 => variables_ui(ui, &m.debug, pc, &(0..(m.cpu.depth() as usize).min(16)).rev().map(|i| m.cpu.stack_entry(i).saturating_sub(1)).collect::<Vec<_>>(), &m.cpu.mem, &m.cpu.prog, self.snap.as_ref()),
                _ => {
                    let (syms, vars, dev, debug) = (&m.syms, &m.debug.vars, &m.dev, &m.debug);
                    watch_ui(ui, &mut self.watch, &mut self.watch_in, |n| syms.get(n).copied().map(|a| (a, false)).or_else(|| vars.iter().find(|v| v.scope.is_none() && !v.rom && v.name == *n).map(|v| (v.addr, false))).or_else(|| debug.path_ref(n).map(|p| (p.0, p.2))).or_else(|| dev.find_sfr(n).map(|s| (s.addr, false))), |a, l, rom| if rom { debug.rom_bytes(&m.cpu.prog, a, l) } else { m.peek(a, l) });
                }
            }
        });
        egui::CentralPanel::default().show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.heading("Program");
                if !self.lines.is_empty() {
                    ui.selectable_value(&mut self.src_view, true, "Source");
                    ui.selectable_value(&mut self.src_view, false, "Disassembly");
                }
                ui.checkbox(&mut self.follow, "follow PC");
                if self.src_view && !self.lines.is_empty() {
                    ui.checkbox(&mut self.stay_home, "stay in my code").on_hover_text("while the PC is in a library routine (DelaySeconds ...), show the line of your program that called it");
                }
                ui.add(egui::TextEdit::singleline(&mut self.goto_in).hint_text("goto 0x… (word)").desired_width(100.0));
                if ui.button("Go").clicked()
                    && let Some(a) = hex_u32(&self.goto_in)
                {
                    self.view_addr = a & 0x7FFF; // the program counter is 15 bits wide; a larger value would overflow the listing's range
                    self.follow = false;
                }
            });
            if self.lines.is_empty() {
                ui.colored_label(egui::Color32::from_rgb(255, 150, 80), "No source line information for this program. Source, Line stepping and the call stack with lines need an ELF with source lines beside the .hex (build with basil -g or xc8-cc -g).");
            }
            if self.src_view && !self.lines.is_empty() {
                let home = std::path::Path::new(self.hex_path.trim()).file_stem().map(|s| s.to_string_lossy().trim_end_matches("_debug").to_ascii_lowercase());
                let callers: Vec<u32> = (0..(m.cpu.depth() as usize).min(16)).rev().map(|i| m.cpu.stack_entry(i).saturating_sub(1)).collect(); // the CALL is the word before the return address
                source_pane(ui, &self.lines, &self.elf_dir, &mut self.sources, &mut self.last_src, &mut m.breakpoints, &callers, pc, self.follow, if self.stay_home { home } else { None });
                return;
            }
            // Every instruction is one 14-bit word at a word address, so the listing can start a few words before the PC.
            let start = if self.follow { pc.saturating_sub(4) } else { self.view_addr };
            let lines = ((ui.available_height() / 15.0) as usize).clamp(8, 80);
            let mut bp_toggle = None;
            for a in start..start + lines as u32 {
                let w = m.cpu.fetch(a);
                if let Some(l) = self.labels.get(&a) {
                    ui.colored_label(egui::Color32::from_rgb(120, 170, 255), format!("{l}:"));
                }
                let has_bp = m.breakpoints.contains(&a);
                let mut rt = egui::RichText::new(format!("{} {a:04X}  {w:04X}  {}", if has_bp { "●" } else { " " }, m.cpu.disasm(w))).monospace();
                if a == pc {
                    rt = rt.background_color(egui::Color32::from_rgb(70, 90, 20)).color(egui::Color32::WHITE);
                } else if has_bp {
                    rt = rt.color(egui::Color32::from_rgb(255, 110, 110));
                }
                if ui.add(egui::Label::new(rt).sense(egui::Sense::click())).clicked() {
                    bp_toggle = Some(a);
                }
            }
            if let Some(b) = bp_toggle {
                if let Some(p) = m.breakpoints.iter().position(|x| *x == b) {
                    m.breakpoints.remove(p);
                } else {
                    m.breakpoints.push(b);
                }
            }
            ui.label("click an instruction to toggle a breakpoint");
        });
    }
}

/// The LEDs (brightness 0..1 in `duty`) and OLEDs of the board, drawn.
fn board_strip(ui: &mut egui::Ui, b: &Board, duty: &[f32]) {
                ui.horizontal_wrapped(|ui| {
                    for (l, d) in b.leds.iter().zip(duty) {
                        ui.vertical(|ui| {
                            let (r, _) = ui.allocate_exact_size(egui::vec2(26.0, 26.0), egui::Sense::hover());
                            let v = (60.0 + 195.0 * d.clamp(0.0, 1.0)) as u8;
                            ui.painter().circle_filled(r.center(), 11.0, egui::Color32::from_rgb(v, (f32::from(v) * 0.15) as u8, (f32::from(v) * 0.1) as u8));
                            ui.label(format!("{} {}", l.name, pin_name(l.pin)));
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
                            ui.label(format!("OLED {} SCL {} SDA {}{st}", match o.bus { Bus::Gpio => "GPIO".to_string(), Bus::I2c(n) => format!("I2C{n}") }, pin_name(o.scl), pin_name(o.sda)));
                        });
                    }
                });
}

/// The BASIC source with the line the PC is on highlighted; a click toggles a breakpoint on that line.
#[allow(clippy::too_many_arguments)]
fn source_pane(ui: &mut egui::Ui, lines: &LineMap, dir: &std::path::Path, cache: &mut std::collections::HashMap<PathBuf, Vec<String>>, last: &mut Option<(PathBuf, u32)>, breakpoints: &mut Vec<u32>, callers: &[u32], pc: u32, follow: bool, home: Option<String>) {
    let mut here = lines.lookup(pc).map(|(f, l)| (f.clone(), l));
    // In a library routine: the nearest caller on the return stack that is in the user's own file.
    if let (Some(h), Some((f, _))) = (&home, &here) {
        let is_home = |p: &PathBuf| p.file_stem().is_some_and(|s| s.to_string_lossy().eq_ignore_ascii_case(h));
        if !is_home(f) && lines.source_files().into_iter().any(|p| is_home(p)) {
            if let Some(c) = callers.iter().find_map(|a| lines.lookup(*a).filter(|(p, _)| is_home(p)).map(|(p, l)| (p.clone(), l))) {
                here = Some(c);
            }
        }
    }
    if let Some(h) = &here {
        *last = Some(h.clone());
    }
    let on_line = here.is_some();
    let Some((file, cur)) = last.clone() else {
        ui.label("The PC is in generated start-up code, not on a BASIC line yet. Step or Run.");
        return;
    };
    if !cache.contains_key(&file) {
        let name = file.file_name().unwrap_or_default();
        let found = [file.clone(), dir.join(name), dir.join("..").join(name)].into_iter().find_map(|p| std::fs::read_to_string(p).ok());
        cache.insert(file.clone(), found.map_or_else(|| vec![format!("(source {} not found)", file.display())], |t| t.lines().map(str::to_string).collect()));
    }
    let src = &cache[&file];
    let bp_lines: Vec<u32> = breakpoints.iter().filter_map(|a| lines.lookup(*a).filter(|(f, _)| **f == file).map(|(_, l)| l)).collect();
    ui.horizontal(|ui| {
        ui.monospace(format!("{}:{cur}", file.file_name().unwrap_or_default().to_string_lossy()));
        if !on_line {
            ui.weak("(PC is in generated code; showing the last source line)");
        }
    });
    let mut toggle = None;
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        for (i, t) in src.iter().enumerate() {
            let n = i as u32 + 1;
            let bp = bp_lines.contains(&n);
            let mut rt = egui::RichText::new(format!("{} {n:>4}  {t}", if bp { "●" } else { " " })).monospace();
            if n == cur {
                rt = rt.background_color(egui::Color32::from_rgb(70, 90, 20)).color(egui::Color32::WHITE);
            } else if bp {
                rt = rt.color(egui::Color32::from_rgb(255, 110, 110));
            }
            let r = ui.add(egui::Label::new(rt).sense(egui::Sense::click()));
            if n == cur && follow {
                r.scroll_to_me(Some(egui::Align::Center));
            }
            if r.clicked() {
                toggle = Some(n);
            }
        }
    });
    if let Some(n) = toggle {
        match lines.first_addr(&file, n) {
            Some(a) => {
                if let Some(p) = breakpoints.iter().position(|x| lines.lookup(*x).is_some_and(|(f, l)| *f == file && l == n)) {
                    breakpoints.remove(p);
                } else {
                    breakpoints.push(a);
                }
            }
            None => {} // a comment or blank line: no code there
        }
    }
}

/// The Board tab: LEDs and an OLED on chosen pins. Returns true when a part or pin changed.
fn board_ui(ui: &mut egui::Ui, board: &mut Board, pins: &[Pin]) -> bool {
            let mut changed = false;
            let mut del = None;
            let pin_box = |ui: &mut egui::Ui, id: String, p: &mut Pin| -> bool {
                let before = *p;
                egui::ComboBox::from_id_salt(id).width(64.0).selected_text(pin_name(*p)).show_ui(ui, |ui| {
                    for q in pins {
                        ui.selectable_value(p, *q, pin_name(*q));
                    }
                });
                *p != before
            };
            ui.heading("LEDs");
            for (i, l) in board.leds.iter_mut().enumerate() {
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
                board.leds.remove(i);
                changed = true;
            }
            if ui.button("Add LED").clicked() {
                let n = board.leds.len() + 1;
                let p = pins.first().copied().unwrap_or((b'A', 0));
                board.leds.push(Led::new(&format!("LED{n}"), p, true));
                changed = true;
            }
            ui.separator();
            ui.heading("OLED (SSD1306, I2C)");
            let mut remove = false;
            for (i, o) in board.oleds.iter_mut().enumerate() {
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
                ui.horizontal(|ui| {
                    ui.checkbox(&mut o.external_vcc, "external VCC (no charge pump needed)");
                    if o.on && !o.powered() {
                        ui.colored_label(egui::Color32::from_rgb(255, 150, 80), "on, but dark: charge pump (8Dh 14h) not enabled");
                    }
                });
                if !o.status.is_empty() {
                    ui.colored_label(egui::Color32::from_rgb(255, 150, 80), format!("not answering: {}", o.status));
                }
            }
            if remove {
                board.oleds.clear();
                changed = true;
            }
            if board.oleds.is_empty() && ui.button("Add OLED").clicked() {
                let a = pins.iter().find(|p| **p == (b'C', 3)).copied().unwrap_or(pins[0]);
                let d = pins.iter().find(|p| **p == (b'C', 4)).copied().unwrap_or(pins[0]);
                board.oleds.push(Oled::new(Bus::I2c(1), a, d, 0x3C, 64));
                changed = true;
            }
            ui.label("The pins must be the ones the program uses. Hardware I2C also needs the program's PPS setup (SSPxCLKPPS/SSPxDATPPS and RxyPPS) to match.");
    changed
}

fn main() -> eframe::Result {
    let mut hex = None;
    let mut device = None;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        if a == "--version" || a == "-V" {
            println!("pic-sim {}", env!("CARGO_PKG_VERSION"));
            return Ok(());
        } else if a == "--device" {
            device = args.next();
        } else {
            hex = Some(a);
        }
    }
    let opts = eframe::NativeOptions { viewport: egui::ViewportBuilder::default().with_inner_size([1280.0, 800.0]).with_title(concat!("pic-sim ", env!("CARGO_PKG_VERSION"))), ..Default::default() };
    eframe::run_native("pic-sim", opts, Box::new(|_| Ok(Box::new(App::new(hex, device)))))
}


#[cfg(test)]
mod stopwatch_tests {
    use super::Stopwatch;

    #[test]
    fn counts_from_start_and_skips_the_stopped_interval() {
        let mut sw = Stopwatch::default();
        sw.start(100);
        assert_eq!(sw.elapsed(160), 60);
        sw.stop(160);
        assert_eq!(sw.elapsed(500), 60); // frozen while stopped
        sw.start(500);
        assert_eq!(sw.elapsed(540), 100); // 60 + 40; the 340 cycles stopped are left out
        sw.clear();
        assert_eq!(sw.elapsed(540), 0); // cleared
        assert!(!sw.counting);
    }
}

#[cfg(test)]
mod caller_frame_tests {
    use super::caller_frames;
    use picsim::debuginfo::{DebugInfo, Form, Func, Kind, Ty, Var};

    fn var(name: &str, addr: u32, scope: Option<usize>) -> Var {
        let layout = Ty { bytes: 1, elem: 1, kind: Kind::Unsigned, array: false, name: "unsigned char".into(), form: Form::Scalar };
        Var { name: name.into(), addr, bytes: 1, kind: Kind::Unsigned, elem: 1, array: false, ty: "unsigned char".into(), scope, param: false, rom: false, layout }
    }

    /// main 0x100..0x200 and mid 0x200..0x300 and leaf 0x300..0x400 have variables; empty 0x400..0x500 has none.
    fn info() -> DebugInfo {
        let f = |n: &str, low, high| Func { name: n.into(), low, high };
        DebugInfo {
            funcs: vec![f("main", 0x100, 0x200), f("mid", 0x200, 0x300), f("leaf", 0x300, 0x400), f("empty", 0x400, 0x500)],
            vars: vec![var("g", 0x20, None), var("m", 0x30, Some(0)), var("x", 0x31, Some(1)), var("y", 0x32, Some(2))],
            ..DebugInfo::default()
        }
    }

    #[test]
    fn callers_come_innermost_first_without_the_current_routine() {
        // In leaf, called from mid, called from main.
        assert_eq!(caller_frames(&info(), Some(2), &[0x250, 0x150]), vec![1, 0]);
    }

    #[test]
    fn a_recursive_routine_and_the_current_one_appear_once_or_not_at_all() {
        // In mid, called from mid (recursion), called from main: mid is current, so only main is a caller.
        assert_eq!(caller_frames(&info(), Some(1), &[0x250, 0x250, 0x150]), vec![0]);
        // In leaf, mid recursing twice: mid once.
        assert_eq!(caller_frames(&info(), Some(2), &[0x250, 0x250, 0x150]), vec![1, 0]);
    }

    #[test]
    fn routines_without_variables_and_unknown_addresses_are_skipped() {
        assert_eq!(caller_frames(&info(), Some(0), &[0x450, 0x900, 0x250]), vec![1]);
        assert!(caller_frames(&info(), None, &[]).is_empty());
    }
}
