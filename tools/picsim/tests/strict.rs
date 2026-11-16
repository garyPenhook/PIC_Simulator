//! Exercise the real CLI with tiny firmware images, without an external compiler.
use std::{
    path::PathBuf,
    process::{Command, Output},
    sync::atomic::{AtomicUsize, Ordering},
};

struct Firmware(PathBuf);
impl Firmware {
    fn new(words: &[u16]) -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "picsim-strict-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&dir).unwrap();
        let bytes: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        let sum = bytes
            .iter()
            .fold(bytes.len() as u8, |sum, b| sum.wrapping_add(*b));
        let record = format!(
            ":{:02X}000000{}{:02X}\n:00000001FF\n",
            bytes.len(),
            bytes.iter().map(|b| format!("{b:02X}")).collect::<String>(),
            sum.wrapping_neg()
        );
        std::fs::write(dir.join("program.hex"), record).unwrap();
        Self(dir)
    }
    fn run(&self, part: &str, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_basil-sim"))
            .arg(self.0.join("program.hex"))
            .args(["--device", part, "--max-cycles", "12"])
            .args(args)
            .output()
            .unwrap()
    }
}
impl Drop for Firmware {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn strict_fails_on_unmodelled_reads_and_writes_in_both_families() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../devices");
    for (part, name, pic16) in [("18F16Q41", "NCO1CON", false), ("16F18326", "CLC1CON", true)] {
        let dev = devdesc::Device::load(&root.join(format!("{part}.toml"))).unwrap();
        let a = dev.find_sfr(name).unwrap().addr as u16;
        for write in [false, true] {
            let words = if pic16 {
                vec![
                    0x0020 | (a >> 7),
                    if write {
                        0x0080 | (a & 0x7f)
                    } else {
                        0x0800 | (a & 0x7f)
                    },
                    0x0001,
                ]
            } else {
                vec![
                    0x0100 | (a >> 8),
                    if write {
                        0x6f00 | (a & 0xff)
                    } else {
                        0x5100 | (a & 0xff)
                    },
                    0x00ff,
                ]
            };
            let fw = Firmware::new(&words);
            for trace in [None, Some("0-2"), Some("0-12"), Some("8-12")] {
                let mut args = vec!["--strict"];
                if let Some(range) = trace {
                    args.extend(["--trace", range]);
                }
                let out = fw.run(part, &args);
                assert_eq!(
                    out.status.code(),
                    Some(3),
                    "{part} {name} write={write} trace={trace:?}: {}",
                    String::from_utf8_lossy(&out.stderr)
                );
                assert!(String::from_utf8_lossy(&out.stderr).contains(name));
                if trace.is_none() {
                    assert!(
                        String::from_utf8_lossy(&out.stdout)
                            .contains("stop: Unmodelled  cycles: 2  instructions: 2"),
                        "must stop before the following RESET instruction: {}",
                        String::from_utf8_lossy(&out.stdout)
                    );
                }
            }
            std::fs::write(
                fw.0.join("program.S"),
                if pic16 {
                    ";SYM done 0x2\n"
                } else {
                    ";SYM done 0x4\n"
                },
            )
            .unwrap();
            let out = fw.run(part, &["--until", "done", "--strict"]);
            assert_eq!(
                out.status.code(),
                Some(3),
                "strict failure must take precedence over reaching the stop symbol"
            );
            let out = fw.run(part, &["--until", "done"]);
            assert!(out.status.success());
            let out = fw.run(part, &[]);
            assert!(
                out.status.success(),
                "default mode must remain permissive: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        }
    }
}

#[test]
fn strict_allows_modelled_firmware() {
    for part in ["18F16Q41", "16F18326"] {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../devices");
        let dev = devdesc::Device::load(&root.join(format!("{part}.toml"))).unwrap();
        let a = dev.find_sfr("STATUS").unwrap().addr as u16;
        let mut words = if part.starts_with("16") {
            vec![0x0020 | (a >> 7), 0x0800 | (a & 0x7f)]
        } else {
            vec![0x0100 | (a >> 8), 0x5100 | (a & 0xff)]
        };
        words.resize(12, 0);
        let fw = Firmware::new(&words);
        let out = fw.run(part, &["--strict"]);
        assert!(
            out.status.success(),
            "{part}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}
