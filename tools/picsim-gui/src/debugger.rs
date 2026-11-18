use eframe::egui;
use picsim::debugger::{Debugger, Signal};
use picsim::periph::{Pin, parse_pin, pin_name};

pub enum Action {
    Write(u32, u8),
    Pc(u32),
    Pin(Pin, bool),
    Adc(u8, u16),
}

pub struct Tools {
    target: String,
    value: u32,
    pin: String,
    high: bool,
    channel: u8,
    adc: u16,
    message: String,
    /// Visible time window of the Waves tab in ns since the capture began; `None` shows the whole capture.
    view: Option<(f64, f64)>,
}

impl Default for Tools {
    fn default() -> Self {
        Self {
            target: "WREG".into(),
            value: 0,
            pin: "RA0".into(),
            high: false,
            channel: 0,
            adc: 0,
            message: String::new(),
            view: None,
        }
    }
}

impl Tools {
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        d: &mut Debugger,
        idle: bool,
        pins: &[Pin],
        resolve: impl Fn(&str) -> Option<u32>,
        peek: impl Fn(u32) -> u8,
        pc: u32,
    ) -> Vec<Action> {
        let mut actions = Vec::new();
        ui.label("Registers and data (hex values)");
        ui.add(
            egui::TextEdit::singleline(&mut self.target).hint_text("SFR, variable, address or PC"),
        );
        let addr = resolve(self.target.trim());
        let is_pc = self.target.trim().eq_ignore_ascii_case("PC");
        if is_pc {
            ui.monospace(format!("PC = {pc:06X}"));
        } else if let Some(a) = addr {
            ui.monospace(format!("{a:04X} = {:02X}", peek(a)));
        } else {
            ui.label("Unknown data address");
        }
        ui.horizontal(|ui| {
            ui.add(egui::DragValue::new(&mut self.value).hexadecimal(2, false, true));
            if ui
                .add_enabled(
                    idle && (is_pc || (addr.is_some() && self.value <= 255)),
                    egui::Button::new("Write"),
                )
                .clicked()
            {
                actions.push(if is_pc {
                    Action::Pc(self.value)
                } else {
                    Action::Write(addr.unwrap(), self.value as u8)
                });
            }
        });
        ui.add_enabled_ui(idle, |ui| {
            if ui
                .add_enabled(
                    addr.is_some() && !is_pc,
                    egui::Button::new("Break on value change"),
                )
                .clicked()
            {
                let a = addr.unwrap();
                if !d.watchpoints.contains(&a) {
                    d.watchpoints.push(a);
                }
            }
        });
        let mut remove = None;
        for (i, a) in d.watchpoints.iter().enumerate() {
            ui.horizontal(|ui| {
                ui.monospace(format!("{a:04X} = {:02X}", peek(*a)));
                if ui.add_enabled(idle, egui::Button::new("Remove")).clicked() {
                    remove = Some(i);
                }
            });
        }
        if let Some(i) = remove {
            d.watchpoints.remove(i);
        }
        ui.small(
            "Stops after an instruction changes a watched byte, including peripheral changes.",
        );
        ui.separator();
        ui.label("External inputs");
        ui.add_enabled_ui(idle, |ui| {
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut self.pin).desired_width(55.0));
                ui.checkbox(&mut self.high, "High");
                if ui.button("Drive pin").clicked() {
                    if let Some(p) = parse_pin(&self.pin).filter(|p| pins.contains(p)) {
                        actions.push(Action::Pin(p, self.high));
                        self.message = format!(
                            "{} driven {} from the current cycle",
                            pin_name(p),
                            if self.high { "high" } else { "low" }
                        );
                    } else {
                        self.message = "Select a pin present on this part (e.g. RA0)".into();
                    }
                }
            });
            ui.horizontal(|ui| {
                ui.label("ADC channel");
                ui.add(egui::DragValue::new(&mut self.channel));
                ui.label("code");
                ui.add(egui::DragValue::new(&mut self.adc));
                if ui.button("Apply").clicked() {
                    actions.push(Action::Adc(self.channel, self.adc));
                }
            });
        });
        ui.small("Inputs use the existing stimulus model and last until overridden or Reset. ADC codes use the part's resolution.");
        ui.label(&self.message);
        ui.separator();
        ui.label("Waveform capture");
        ui.add_enabled_ui(idle && !d.recording && d.samples.is_empty(), |ui| {
            if ui
                .add_enabled(
                    addr.is_some() && !is_pc,
                    egui::Button::new("Trace data byte"),
                )
                .clicked()
            {
                d.signals.push(Signal {
                    name: self.target.trim().into(),
                    addr: addr.unwrap(),
                    bit: None,
                });
            }
            if ui.button("Trace pin").clicked() {
                if let Some(p) = parse_pin(&self.pin).filter(|p| pins.contains(p)) {
                    if let Some(a) = resolve(&format!("PORT{}", p.0 as char)) {
                        d.signals.push(Signal {
                            name: pin_name(p),
                            addr: a,
                            bit: Some(p.1),
                        });
                    }
                } else {
                    self.message = "Choose an existing pin above before tracing".into();
                }
            }
        });
        let mut remove = None;
        for (i, s) in d.signals.iter().enumerate() {
            ui.horizontal(|ui| {
                ui.label(&s.name);
                if ui
                    .add_enabled(
                        idle && !d.recording && d.samples.is_empty(),
                        egui::Button::new("Remove trace"),
                    )
                    .clicked()
                {
                    remove = Some(i);
                }
            });
        }
        if let Some(i) = remove {
            d.signals.remove(i);
        }
        ui.horizontal(|ui| {
            if ui
                .add_enabled(
                    idle && !d.signals.is_empty() && !d.full,
                    egui::Button::new(if d.recording {
                        "Stop capture"
                    } else {
                        "Record"
                    }),
                )
                .clicked()
            {
                d.recording = !d.recording;
            }
            if ui
                .add_enabled(idle, egui::Button::new("Clear capture"))
                .clicked()
            {
                d.reset();
                d.recording = false;
            }
            if ui
                .add_enabled(!d.samples.is_empty(), egui::Button::new("Export VCD…"))
                .clicked()
            {
                if let Some(path) = rfd::FileDialog::new()
                    .add_filter("VCD waveform", &["vcd"])
                    .set_file_name("waveform.vcd")
                    .save_file()
                {
                    self.message = match std::fs::write(&path, d.vcd()) {
                        Ok(()) => format!("Exported {}", path.display()),
                        Err(e) => format!("VCD export failed: {e}"),
                    };
                }
            }
        });
        ui.small(format!(
            "{} samples / {} maximum{}",
            d.samples.len(),
            Debugger::MAX_SAMPLES,
            if d.full {
                " — capture full, clear to restart"
            } else {
                ""
            }
        ));
        ui.small("Samples at instruction boundaries; short pulses within an instruction may be missed. Pin traces show digital PORT readings (ANSEL applies).");
        ui.small("The captured waveform is drawn in the Waves tab.");
        actions
    }

    pub fn input_result(&mut self, ok: bool) {
        self.message = if ok {
            "Input applied at the current cycle"
        } else {
            "This input is unavailable on this part"
        }
        .into();
    }
}

/// A time as ns / us / ms.
fn time_text(ns: f64) -> String {
    let a = ns.abs();
    if a >= 1e6 {
        format!("{:.3} ms", ns / 1e6)
    } else if a >= 1e3 {
        format!("{:.3} us", ns / 1e3)
    } else {
        format!("{ns:.0} ns")
    }
}

/// A tick spacing of 1, 2 or 5 times a power of ten, giving about `want` ticks over `span`.
fn tick_step(span: f64, want: f64) -> f64 {
    let raw = (span / want).max(1.0);
    let p = 10f64.powf(raw.log10().floor());
    [1.0, 2.0, 5.0, 10.0].iter().map(|m| m * p).find(|s| *s >= raw).unwrap_or(raw)
}

/// Value of signal `i` at `t` ns after the capture began (each sample holds until the next).
fn value_at(d: &Debugger, i: usize, t: f64, base: u64) -> u8 {
    let k = d.samples.partition_point(|s| (s.ns - base) as f64 <= t);
    d.samples[k.saturating_sub(1)].values[i]
}

impl Tools {
    /// The Waves tab: capture controls and an interactive plot (scroll or the buttons to zoom, drag to pan, hover to read).
    pub fn waves(&mut self, ui: &mut egui::Ui, d: &mut Debugger, idle: bool) {
        ui.horizontal_wrapped(|ui| {
            if ui.add_enabled(idle && !d.signals.is_empty() && !d.full, egui::Button::new(if d.recording { "Stop capture" } else { "Record" })).clicked() {
                d.recording = !d.recording;
            }
            if ui.add_enabled(idle, egui::Button::new("Clear capture")).clicked() {
                d.reset();
                d.recording = false;
                self.view = None;
            }
            if ui.add_enabled(!d.samples.is_empty(), egui::Button::new("Export VCD…")).clicked()
                && let Some(path) = rfd::FileDialog::new().add_filter("VCD waveform", &["vcd"]).set_file_name("waveform.vcd").save_file()
            {
                self.message = match std::fs::write(&path, d.vcd()) {
                    Ok(()) => format!("Exported {}", path.display()),
                    Err(e) => format!("VCD export failed: {e}"),
                };
            }
        });
        if d.signals.is_empty() {
            ui.label("No signals chosen. In the Debugger tab, pick a pin or data byte and press \"Trace pin\" / \"Trace data byte\", then Record here.");
            return;
        }
        ui.small(format!("{} samples / {} maximum{}{}", d.samples.len(), Debugger::MAX_SAMPLES, if d.recording { " - recording" } else { "" }, if d.full { " - capture full, clear to restart" } else { "" }));
        if !self.message.is_empty() {
            ui.small(&self.message);
        }
        let (Some(first), Some(last)) = (d.samples.first(), d.samples.last()) else {
            self.view = None;
            ui.label("Press Record, then Run or Step: samples are taken at instruction boundaries.");
            return;
        };
        let base = first.ns;
        let total = last.ns.saturating_sub(base).max(1) as f64;
        let (mut t0, mut t1) = self.view.unwrap_or((0.0, total));
        ui.horizontal(|ui| {
            let mut zoom = None;
            if ui.button("Zoom in").clicked() { zoom = Some(0.5); }
            if ui.button("Zoom out").clicked() { zoom = Some(2.0); }
            if ui.button("Fit").clicked() { self.view = None; (t0, t1) = (0.0, total); }
            ui.label(format!("{} captured, showing {}", time_text(total), time_text(t1 - t0)));
            if let Some(f) = zoom {
                let mid = (t0 + t1) / 2.0;
                let half = (t1 - t0) * f / 2.0;
                (t0, t1) = (mid - half, mid + half);
            }
        });
        let label_w = 70.0;
        let lane = 40.0;
        let axis = 18.0;
        let width = (ui.available_width() - label_w).max(60.0);
        let height = axis + lane * d.signals.len() as f32;
        let (rect, resp) = ui.allocate_exact_size(egui::vec2(ui.available_width(), height), egui::Sense::click_and_drag());
        let plot = egui::Rect::from_min_size(rect.min + egui::vec2(label_w, 0.0), egui::vec2(width, height));
        // Pan with a drag, zoom about the pointer with the scroll wheel.
        let per_px = (t1 - t0) / f64::from(width);
        if resp.dragged() {
            let dt = -f64::from(resp.drag_delta().x) * per_px;
            (t0, t1) = (t0 + dt, t1 + dt);
        }
        if resp.hovered() {
            let scroll = ui.input(|i| i.smooth_scroll_delta.y + i.smooth_scroll_delta.x);
            if scroll != 0.0 && let Some(p) = resp.hover_pos() {
                ui.input_mut(|i| i.smooth_scroll_delta = egui::Vec2::ZERO); // the panel must not scroll under the plot
                let at = t0 + f64::from(p.x - plot.left()) * per_px;
                let f = (-f64::from(scroll) * 0.004).exp();
                (t0, t1) = (at - (at - t0) * f, at + (t1 - at) * f);
            }
        }
        // Keep the window inside the capture and at least 10 ns wide.
        let span = (t1 - t0).clamp(10.0, total.max(10.0));
        t0 = t0.clamp(0.0, (total - span).max(0.0));
        t1 = t0 + span;
        self.view = if t0 <= 0.0 && t1 >= total { None } else { Some((t0, t1)) };
        let x = |t: f64| plot.left() + ((t - t0) / span) as f32 * plot.width();
        let painter = ui.painter_at(rect);
        let fg = ui.visuals().text_color();
        let weak = ui.visuals().weak_text_color();
        let trace = ui.visuals().selection.bg_fill;
        let grid = ui.visuals().widgets.noninteractive.bg_stroke.color;
        let font = egui::TextStyle::Small.resolve(ui.style());
        // Time axis.
        let step = tick_step(span, f64::from(width) / 90.0);
        let mut t = (t0 / step).ceil() * step;
        while t <= t1 {
            let px = x(t);
            painter.line_segment([egui::pos2(px, plot.top() + axis - 4.0), egui::pos2(px, plot.bottom())], egui::Stroke::new(1.0, grid));
            painter.text(egui::pos2(px + 2.0, plot.top()), egui::Align2::LEFT_TOP, time_text(t), font.clone(), weak);
            t += step;
        }
        let first_k = d.samples.partition_point(|s| ((s.ns - base) as f64) < t0).saturating_sub(1);
        let last_k = d.samples.partition_point(|s| ((s.ns - base) as f64) <= t1).min(d.samples.len());
        for (i, sig) in d.signals.iter().enumerate() {
            let top = plot.top() + axis + lane * i as f32 + 4.0;
            let bot = top + lane - 12.0;
            painter.text(egui::pos2(rect.left() + 2.0, (top + bot) / 2.0), egui::Align2::LEFT_CENTER, &sig.name, font.clone(), fg);
            let bit = sig.bit.is_some();
            let level = |v: u8| if v != 0 { top } else { bot };
            // Bound the drawing work on a huge window; the export keeps every sample.
            let stride = (last_k - first_k).div_ceil(4000).max(1);
            let samples: Vec<&picsim::debugger::Sample> = d.samples[first_k..last_k].iter().step_by(stride).collect();
            let mut seg_start = t0;
            let mut cur = samples.first().map_or(0, |s| s.values[i]);
            let stroke = egui::Stroke::new(1.5, trace);
            let close = |from: f64, to: f64, v: u8, painter: &egui::Painter| {
                let (a, b) = (x(from.max(t0)), x(to.min(t1)));
                if bit {
                    painter.line_segment([egui::pos2(a, level(v)), egui::pos2(b, level(v))], stroke);
                } else {
                    // A byte is a bus: a box with its hex value when it is wide enough to read.
                    let m = ((top + bot) / 2.0, 3.0f32.min((b - a) / 2.0));
                    painter.line_segment([egui::pos2(a + m.1, top), egui::pos2(b - m.1, top)], stroke);
                    painter.line_segment([egui::pos2(a + m.1, bot), egui::pos2(b - m.1, bot)], stroke);
                    painter.line_segment([egui::pos2(a, m.0), egui::pos2(a + m.1, top)], stroke);
                    painter.line_segment([egui::pos2(a, m.0), egui::pos2(a + m.1, bot)], stroke);
                    if b - a > 24.0 {
                        painter.text(egui::pos2(a + 6.0, m.0), egui::Align2::LEFT_CENTER, format!("{v:02X}"), font.clone(), fg);
                    }
                }
            };
            for s in &samples {
                let v = s.values[i];
                if v != cur {
                    let at = (s.ns - base) as f64;
                    close(seg_start, at, cur, &painter);
                    if bit {
                        let px = x(at);
                        painter.line_segment([egui::pos2(px, level(cur)), egui::pos2(px, level(v))], stroke);
                    }
                    seg_start = at;
                    cur = v;
                }
            }
            close(seg_start, t1, cur, &painter);
            painter.line_segment([egui::pos2(plot.left(), bot + 4.0), egui::pos2(plot.right(), bot + 4.0)], egui::Stroke::new(0.5, grid));
        }
        // Cursor: the time under the pointer and each signal's value there.
        if let Some(p) = resp.hover_pos().filter(|p| plot.contains(*p)) {
            let at = (t0 + f64::from(p.x - plot.left()) * span / f64::from(width)).clamp(0.0, total);
            painter.line_segment([egui::pos2(p.x, plot.top() + axis - 4.0), egui::pos2(p.x, plot.bottom())], egui::Stroke::new(1.0, fg));
            let mut tip = format!("t = {}", time_text(at));
            for (i, sig) in d.signals.iter().enumerate() {
                let v = value_at(d, i, at, base);
                tip.push_str(&format!("\n{} = {}", sig.name, if sig.bit.is_some() { v.to_string() } else { format!("0x{v:02X} ({v})") }));
            }
            resp.on_hover_text(tip);
        }
    }
}
