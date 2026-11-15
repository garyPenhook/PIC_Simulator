use eframe::egui;
use picsim::{log::Log, periph::RxStream};

const DISPLAY_BYTES: usize = 16_384;
const INPUT_BYTES: usize = 4096;

#[derive(Default)]
pub(super) struct Terminal {
    port: u8,
    input: String,
    hex: bool,
    ending: usize,
    cleared: [u64; 3],
    status: String,
}

impl Terminal {
    pub fn clear(&mut self) {
        self.cleared = [0; 3];
        self.status.clear();
    }

    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        tx: &Log<(u8, u64, u8)>,
        ports: &[u8],
        now: u64,
    ) -> Option<(u8, RxStream)> {
        ui.heading("UART terminal");
        if ports.is_empty() {
            ui.label("This part has no modelled UART.");
            return None;
        }
        if !ports.contains(&self.port) {
            self.port = ports[0];
        }
        ui.horizontal(|ui| {
            egui::ComboBox::from_id_salt("terminal_port")
                .selected_text(format!("UART{}", self.port))
                .show_ui(ui, |ui| {
                    for &port in ports {
                        ui.selectable_value(&mut self.port, port, format!("UART{port}"));
                    }
                });
            ui.checkbox(&mut self.hex, "Hex");
            if ui.button("Clear output").clicked() {
                self.cleared[(self.port - 1) as usize] = tx.total();
            }
        });
        let start = self.cleared[(self.port - 1) as usize];
        let (bytes, omitted) = output_bytes(tx, self.port, start);
        if omitted {
            ui.label("Showing recent output; older output was omitted.");
        }
        let output = display(&bytes, self.hex);
        let mut view = output.as_str();
        egui::ScrollArea::vertical()
            .id_salt("terminal_output")
            .max_height(280.0)
            .stick_to_bottom(true)
            .show(ui, |ui| {
                ui.add(
                    egui::TextEdit::multiline(&mut view)
                        .font(egui::TextStyle::Monospace)
                        .desired_width(f32::INFINITY),
                );
            });
        ui.label(if self.hex {
            "Input: hexadecimal bytes (e.g. 41 00 FF)"
        } else {
            "Input: text (UTF-8 bytes)"
        });
        ui.add(egui::TextEdit::singleline(&mut self.input).char_limit(INPUT_BYTES));
        ui.horizontal(|ui| {
            egui::ComboBox::from_id_salt("terminal_ending")
                .selected_text(["No line ending", "LF", "CRLF"][self.ending])
                .show_ui(ui, |ui| {
                    for (i, name) in ["No line ending", "LF", "CRLF"].iter().enumerate() {
                        ui.selectable_value(&mut self.ending, i, *name);
                    }
                });
        });
        let mut send = None;
        if ui.button("Send").clicked() {
            match input_bytes(&self.input, self.hex, self.ending) {
                Ok(bytes) if !bytes.is_empty() => {
                    self.status =
                        format!("Queued {} bytes. Run or step to receive them.", bytes.len());
                    send = Some((
                        self.port,
                        RxStream {
                            at: now.saturating_add(1),
                            words: bytes.into_iter().map(u16::from).collect(),
                            bit: None,
                            gap: 0,
                            nine: false,
                            bad_stop: false,
                        },
                    ));
                    self.input.clear();
                }
                Ok(_) => self.status = "Enter bytes or select a line ending.".into(),
                Err(e) => self.status = e,
            }
        }
        ui.label(&self.status);
        ui.label("Input uses the firmware's baud rate. Enable its receiver before sending.");
        send
    }
}

/// Read only the display window, keeping work bounded even when the shared log is much larger.
fn output_bytes(tx: &Log<(u8, u64, u8)>, port: u8, start: u64) -> (Vec<u8>, bool) {
    let mut bytes: Vec<u8> = tx
        .since(start)
        .iter()
        .rev()
        .filter(|t| t.0 == port)
        .take(DISPLAY_BYTES + 1)
        .map(|t| t.2)
        .collect();
    let omitted = tx.dropped() > start || bytes.len() > DISPLAY_BYTES;
    bytes.truncate(DISPLAY_BYTES);
    bytes.reverse();
    (bytes, omitted)
}

fn input_bytes(input: &str, hex: bool, ending: usize) -> Result<Vec<u8>, String> {
    let mut bytes = if hex {
        input
            .split_whitespace()
            .map(|word| {
                if word.len() != 2 {
                    return Err(format!("Expected two hex digits per byte: {word}"));
                }
                u8::from_str_radix(word, 16).map_err(|_| format!("Invalid hex byte: {word}"))
            })
            .collect::<Result<Vec<_>, _>>()?
    } else {
        input.as_bytes().to_vec()
    };
    bytes.extend_from_slice(match ending {
        1 => b"\n",
        2 => b"\r\n",
        _ => b"",
    });
    if bytes.len() > INPUT_BYTES {
        return Err(format!("Send at most {INPUT_BYTES} bytes at a time."));
    }
    Ok(bytes)
}

fn display(bytes: &[u8], hex: bool) -> String {
    if hex {
        return bytes
            .iter()
            .map(|b| format!("{b:02X}"))
            .collect::<Vec<_>>()
            .join(" ");
    }
    String::from_utf8_lossy(bytes)
        .chars()
        .map(|c| {
            if c.is_control() && !matches!(c, '\n' | '\r' | '\t') {
                c.escape_default().to_string()
            } else {
                c.to_string()
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_filters_ports_and_survives_clear_and_log_rollover() {
        let mut tx = Log::with_cap(4);
        tx.push((1, 0, b'A'));
        tx.push((2, 0, b'X'));
        let cleared = tx.total();
        tx.push((1, 1, b'B'));
        assert_eq!(output_bytes(&tx, 1, cleared), (b"B".to_vec(), false));
        assert_eq!(output_bytes(&tx, 2, 0), (b"X".to_vec(), false));
        tx.push((1, 2, b'C'));
        tx.push((1, 3, b'D'));
        assert_eq!(output_bytes(&tx, 1, 0), (b"BCD".to_vec(), true));
        assert_eq!(output_bytes(&tx, 1, cleared), (b"BCD".to_vec(), false));
        assert_eq!(output_bytes(&tx, 1, tx.total()), (vec![], false));
        for _ in 0..DISPLAY_BYTES + 1 {
            tx.push((1, 4, b'Z'));
        }
        assert!(output_bytes(&tx, 1, cleared).1);
        let mut tx = Log::new();
        for _ in 0..DISPLAY_BYTES + 1 {
            tx.push((1, 4, b'Z'));
        }
        let (bytes, omitted) = output_bytes(&tx, 1, 0);
        assert_eq!(bytes.len(), DISPLAY_BYTES);
        assert!(omitted);
    }

    #[test]
    fn terminal_encodes_binary_and_text_without_losing_bytes() {
        assert_eq!(input_bytes("41 00 ff", true, 2).unwrap(), b"A\0\xff\r\n");
        assert_eq!(input_bytes("é", false, 1).unwrap(), "é\n".as_bytes());
        assert!(input_bytes("0x41", true, 0).is_err());
        assert!(input_bytes("GG", true, 0).is_err());
        assert!(input_bytes(&"x".repeat(INPUT_BYTES), false, 1).is_err());
        assert_eq!(display(b"A\0\xff", true), "41 00 FF");
        assert_eq!(display(b"A\0\n", false), "A\\u{0}\n");
    }
}
