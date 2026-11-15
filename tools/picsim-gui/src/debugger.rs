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
        waveform(ui, d);
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

fn waveform(ui: &mut egui::Ui, d: &Debugger) {
    let (Some(first), Some(last)) = (d.samples.first(), d.samples.last()) else {
        return;
    };
    ui.label(format!("{} ns captured", last.ns.saturating_sub(first.ns)));
    let span = last.ns.saturating_sub(first.ns).max(1) as f32;
    for (i, s) in d.signals.iter().enumerate() {
        ui.label(&s.name);
        let (rect, _) =
            ui.allocate_exact_size(egui::vec2(ui.available_width(), 32.0), egui::Sense::hover());
        let x = |ns: u64| rect.left() + ns.saturating_sub(first.ns) as f32 / span * rect.width();
        let y = |v: u8| {
            rect.bottom() - 3.0 - f32::from(v) / if s.bit.is_some() { 1.0 } else { 255.0 } * 26.0
        };
        let stroke = egui::Stroke::new(1.0, ui.visuals().selection.bg_fill);
        // Bound drawing work while preserving the full capture in the export.
        let stride = d.samples.len().div_ceil(1500).max(1);
        let mut previous = &d.samples[0];
        for sample in d
            .samples
            .iter()
            .skip(1)
            .step_by(stride)
            .chain(std::iter::once(last))
        {
            let a = egui::pos2(x(previous.ns), y(previous.values[i]));
            let b = egui::pos2(x(sample.ns), a.y);
            ui.painter().line_segment([a, b], stroke);
            ui.painter()
                .line_segment([b, egui::pos2(b.x, y(sample.values[i]))], stroke);
            previous = sample;
        }
    }
}
