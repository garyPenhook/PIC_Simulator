//! Debug Adapter Protocol over stdin/stdout. Stdout contains protocol frames only.
use picsim::{
    LineStep, Machine, Stop,
    debuginfo::{DebugInfo, Form, Frame, Var},
    machine16::Machine16,
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    io::{self, BufRead, Write},
    path::{Path, PathBuf},
    sync::mpsc,
    time::Duration,
};

enum Target {
    P18(Machine),
    P16(Machine16),
}
macro_rules! target {
    ($s:expr, $m:ident, $e:expr) => {
        match $s {
            Target::P18($m) => $e,
            Target::P16($m) => $e,
        }
    };
}
impl Target {
    fn debug(&self) -> &DebugInfo {
        target!(self, m, &m.debug)
    }
    fn pc(&self) -> u32 {
        target!(self, m, m.cpu.pc)
    }
    fn cycles(&self) -> u64 {
        target!(self, m, m.cpu.cycles)
    }
    fn prog(&self) -> &[u8] {
        target!(self, m, &m.cpu.prog)
    }
    fn mem(&self) -> &[u8] {
        match self {
            Self::P18(m) => &m.cpu.m.mem,
            Self::P16(m) => &m.cpu.mem,
        }
    }
    fn peek(&self, addr: u32, len: usize) -> Vec<u8> {
        target!(self, m, m.peek(addr, len))
    }
    fn write(&mut self, addr: u32, bytes: &[u8]) {
        target!(self, m, {
            for (i, b) in bytes.iter().enumerate() {
                m.cpu.write(addr + i as u32, *b);
            }
            m.cpu.settle();
        })
    }
    fn debugger(&mut self) -> &mut picsim::debugger::Debugger {
        target!(self, m, &mut m.debugger)
    }
    /// The data address of `v` in the frame whose base is `base`, as the CPU's memory has it, checked to lie inside data memory.
    /// A PIC16 frame on XC8's software stack is at a linear address (from 2000h), which is translated to its banked one.
    fn data_place(&self, v: &Var, base: Option<u32>) -> Result<u32, String> {
        if v.rom {
            return Err("a constant in program memory cannot be changed".into());
        }
        let a = v.place(base).ok_or("the variable is not in a live frame")?;
        let mem = self.mem().len() as u32;
        let a = if self.debug().word_addressed && mem <= 0x2000 && a >= 0x2000 { picsim::pic16::linear_to_data(a - 0x2000) as u32 } else { a };
        if a.checked_add(v.bytes).is_none_or(|end| end > mem) {
            return Err("the variable is outside data memory".into());
        }
        Ok(a)
    }
    fn sites(&self) -> Vec<u32> {
        target!(self, m, m.call_sites())
    }
    fn frames(&self) -> Vec<Frame> {
        target!(self, m, m.frames())
    }
    fn step(&mut self) -> Option<Stop> {
        target!(self, m, m.step())
    }
    fn sleep_dead(&self) -> bool {
        target!(self, m, m.cpu.sleep_dead())
    }
    fn halt(&self) -> Option<u32> {
        target!(self, m, m.syms.get("__halt").copied())
    }
    fn depth(&self) -> u8 {
        match self {
            Self::P18(m) => m.stack_depth(),
            Self::P16(m) => m.cpu.depth() as u8,
        }
    }
    fn bps(&mut self) -> &mut Vec<u32> {
        target!(self, m, &mut m.breakpoints)
    }
    fn begin(&self, over: bool, out: bool) -> LineStep {
        target!(self, m, m.begin_line_step(over, out))
    }
    fn line(&mut self, s: &LineStep) -> Option<Option<Stop>> {
        target!(self, m, m.line_step(s, 256))
    }
    fn reset(&mut self) -> Result<(), String> {
        match self {
            Self::P18(m) => {
                m.reset();
                Ok(())
            }
            Self::P16(m) => m.reset(),
        }
    }
}

fn read_message(r: &mut impl BufRead) -> Result<Option<Value>, String> {
    let mut length = None;
    let mut total = 0;
    loop {
        let mut line = String::new();
        if r.read_line(&mut line).map_err(|e| e.to_string())? == 0 {
            return if total == 0 {
                Ok(None)
            } else {
                Err("truncated DAP header".into())
            };
        }
        total += line.len();
        if total > 8192 {
            return Err("DAP header too large".into());
        }
        if line == "\r\n" || line == "\n" {
            break;
        }
        if let Some((key, value)) = line.split_once(':') {
            if key.eq_ignore_ascii_case("Content-Length") {
                if length.is_some() {
                    return Err("duplicate Content-Length".into());
                }
                length = Some(
                    value
                        .trim()
                        .parse::<usize>()
                        .map_err(|_| "invalid Content-Length")?,
                );
            }
        }
    }
    let n = length.ok_or("missing Content-Length")?;
    if n > 4 * 1024 * 1024 {
        return Err("DAP message exceeds 4 MiB".into());
    }
    let mut bytes = vec![0; n];
    r.read_exact(&mut bytes).map_err(|e| e.to_string())?;
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|e| e.to_string())
}
struct Wire<W: Write> {
    out: W,
    seq: u64,
}
impl<W: Write> Wire<W> {
    fn send(&mut self, mut v: Value) -> io::Result<()> {
        self.seq += 1;
        v["seq"] = json!(self.seq);
        let bytes = serde_json::to_vec(&v)?;
        write!(self.out, "Content-Length: {}\r\n\r\n", bytes.len())?;
        self.out.write_all(&bytes)?;
        self.out.flush()
    }
    fn event(&mut self, event: &str, body: Value) -> io::Result<()> {
        self.send(json!({"type":"event", "event":event, "body":body}))
    }
    fn response(&mut self, req: &Value, result: Result<Value, String>) -> io::Result<()> {
        let mut v = json!({"type":"response", "request_seq":req["seq"], "command":req["command"], "success":result.is_ok()});
        match result {
            Ok(body) => v["body"] = body,
            Err(e) => v["message"] = json!(e),
        }
        self.send(v)
    }
}
enum Running {
    Continue { skip: bool, depth: u8 },
    Line(LineStep),
    Instruction,
    Over { depth: u8 },
}
#[derive(Default)]
struct Adapter {
    target: Option<Target>,
    running: Option<Running>,
    initialized: bool,
    configured: bool,
    stop_on_entry: bool,
    source_bps: BTreeMap<PathBuf, Vec<u32>>,
    instruction_bps: Vec<u32>,
}
fn string<'a>(a: &'a Value, key: &str) -> Result<&'a str, String> {
    a[key]
        .as_str()
        .ok_or_else(|| format!("{key} must be a string"))
}
/// The variable `name` in the scope `reference` (as `variables` lists them) and the base of the frame it is read in.
fn scope_var(t: &Target, reference: u64, name: &str) -> Result<(Var, Option<u32>), String> {
    let frame = reference / 2;
    if frame == 0 || frame > 1 + t.sites().len() as u64 {
        return Err("invalid variablesReference".into());
    }
    let (d, frames) = (t.debug(), t.frames());
    let f = frames.get(frame as usize - 1);
    let v = d
        .vars
        .iter()
        .filter(|v| if reference % 2 == 1 { v.scope.is_none() } else { v.scope.is_some() && v.scope == f.and_then(|f| f.func) })
        .find(|v| d.var_label(v) == name)
        .ok_or("unknown variable")?;
    Ok((v.clone(), f.and_then(|f| f.base)))
}
/// `dataBreakpointInfo`'s id: `address(hex):bytes:mask(hex)`. A mask other than FF watches only those bits of the byte.
fn parse_data_id(id: &str, size: u32) -> Result<(u32, u32, u8), String> {
    let bad = || "unknown dataId".to_string();
    let mut it = id.split(':');
    let addr = u32::from_str_radix(it.next().ok_or_else(bad)?, 16).map_err(|_| bad())?;
    let len: u32 = it.next().ok_or_else(bad)?.parse().map_err(|_| bad())?;
    let mask = u8::from_str_radix(it.next().ok_or_else(bad)?, 16).map_err(|_| bad())?;
    if it.next().is_some() || len == 0 || mask == 0 || (mask != 0xFF && len != 1) || addr.checked_add(len).is_none_or(|e| e > size) {
        return Err(bad());
    }
    Ok((addr, len, mask))
}
fn address(v: &Value) -> Result<u32, String> {
    if let Some(n) = v.as_u64() {
        return u32::try_from(n).map_err(|_| "address out of range".into());
    }
    let s = v.as_str().ok_or("expected address")?;
    if let Some(s) = s.strip_prefix("0x") {
        u32::from_str_radix(s, 16)
    } else {
        s.parse()
    }
    .map_err(|_| "invalid address".into())
}
fn path(p: &str, cwd: &Path) -> PathBuf {
    let p = Path::new(p);
    let p = if p.is_absolute() {
        p.to_owned()
    } else {
        cwd.join(p)
    };
    p.canonicalize().unwrap_or(p)
}
fn source(p: &Path) -> Value {
    json!({"name":p.file_name().unwrap_or_default().to_string_lossy(), "path":p.to_string_lossy()})
}
impl Adapter {
    fn target(&self) -> Result<&Target, String> {
        self.target.as_ref().ok_or("launch a program first".into())
    }
    fn stopped(&self) -> Result<&Target, String> {
        if self.running.is_some() {
            Err("notStopped".into())
        } else {
            self.target()
        }
    }
    fn rebuild_bps(&mut self) {
        let mut bps: Vec<u32> = self
            .source_bps
            .values()
            .flatten()
            .copied()
            .chain(self.instruction_bps.iter().copied())
            .collect();
        bps.sort_unstable();
        bps.dedup();
        if let Some(t) = &mut self.target {
            *t.bps() = bps;
        }
    }
    fn launch(&mut self, a: &Value) -> Result<Value, String> {
        if self.target.is_some() {
            return Err("disconnect before launching another program".into());
        }
        let cwd = a["cwd"]
            .as_str()
            .map(PathBuf::from)
            .unwrap_or(std::env::current_dir().map_err(|e| e.to_string())?);
        let program = path(string(a, "program")?, &cwd);
        let elf = if program.extension().is_some_and(|e| e == "elf") {
            Some(program.clone())
        } else {
            picsim::debuginfo::elf_for(&program)
        };
        let hex = if program.extension().is_some_and(|e| e == "elf") {
            let p = program.with_extension("hex");
            if p.exists() {
                p
            } else {
                program
                    .parent()
                    .unwrap_or(Path::new("."))
                    .join("dist")
                    .join(program.file_stem().unwrap())
                    .with_extension("hex")
            }
        } else {
            program.clone()
        };
        let detected = elf
            .as_ref()
            .and_then(|p| std::fs::read(p).ok())
            .and_then(|b| picsim::debuginfo::device_of(&b));
        let listed = picsim::cofinfo::device_of_listing(&program);
        let part = a["device"]
            .as_str()
            .or(detected.as_deref())
            .or(listed.as_deref())
            .ok_or("device is required without a device-tagged ELF")?;
        if part.contains(['/', '\\']) || part.contains("..") {
            return Err("invalid device name".into());
        }
        let root = a["devices"]
            .as_str()
            .map(|s| path(s, &cwd))
            .unwrap_or_else(|| {
                devdesc::devices_dir(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../devices"))
            });
        let dev =
            devdesc::Device::load(&root.join(format!("{}.toml", part.trim_start_matches("PIC"))))
                .map_err(|e| e.to_string())?;
        let mut t = if dev.device.family == "pic18" {
            Target::P18(Machine::new(dev)?)
        } else {
            Target::P16(Machine16::new(dev)?)
        };
        let text = std::fs::read_to_string(&hex).map_err(|e| format!("{}: {e}", hex.display()))?;
        target!(&mut t, m, m.load_hex(&text))?;
        let asm = a["asm"]
            .as_str()
            .map(|s| path(s, &cwd))
            .unwrap_or_else(|| hex.with_extension("S"));
        if let Ok(text) = std::fs::read_to_string(asm) {
            target!(&mut t, m, m.load_symbols_from_asm(&text));
        }
        if let Some(elf) = elf {
            target!(&mut t, m, m.load_elf(&elf))?;
        } else if let Some(cof) = picsim::cofinfo::cof_for(&hex) {
            target!(&mut t, m, m.load_cof(&cof))?;
        }
        if let Some(stim) = a["stimulus"].as_str() {
            let text = std::fs::read_to_string(path(stim, &cwd)).map_err(|e| e.to_string())?;
            target!(&mut t, m, m.load_stimulus(&text))?;
        }
        self.target = Some(t);
        self.stop_on_entry = a["stopOnEntry"].as_bool().unwrap_or(true);
        self.configured = false;
        Ok(json!({}))
    }
    fn request(&mut self, command: &str, a: &Value) -> Result<Value, String> {
        if command != "initialize" && !self.initialized {
            return Err("initialize must be first".into());
        }
        match command {
            "initialize" => {
                if self.initialized {
                    return Err("already initialized".into());
                }
                if a["linesStartAt1"] == false
                    || a["columnsStartAt1"] == false
                    || a["pathFormat"].as_str().is_some_and(|s| s != "path")
                {
                    return Err("use one-based lines/columns and native paths".into());
                }
                self.initialized = true;
                Ok(
                    json!({"supportsConfigurationDoneRequest":true, "supportsSteppingGranularity":true,
                          "supportsInstructionBreakpoints":true, "supportsEvaluateForHovers":true,
                          "supportsRestartRequest":true, "supportsTerminateRequest":true, "supportsLoadedSourcesRequest":true,
                          "supportsSetVariable":true, "supportsDataBreakpoints":true}),
                )
            }
            "launch" => self.launch(a),
            "setBreakpoints" => {
                let t = self.target()?;
                let requested = PathBuf::from(string(&a["source"], "path")?);
                let file = path(
                    &requested.to_string_lossy(),
                    &std::env::current_dir().map_err(|e| e.to_string())?,
                );
                let actual = t
                    .debug()
                    .lines
                    .files
                    .iter()
                    .find(|p| path(&p.to_string_lossy(), Path::new(".")) == file)
                    .cloned()
                    .unwrap_or(file.clone());
                let mut addresses = Vec::new();
                let mut replies = Vec::new();
                for bp in a["breakpoints"]
                    .as_array()
                    .ok_or("breakpoints must be an array")?
                {
                    let line = bp["line"]
                        .as_u64()
                        .and_then(|n| u32::try_from(n).ok())
                        .unwrap_or(0);
                    let unsupported = ["condition", "hitCondition", "logMessage"]
                        .iter()
                        .any(|k| !bp[k].is_null());
                    let addr = (!unsupported)
                        .then(|| t.debug().lines.first_addr(&actual, line))
                        .flatten();
                    if let Some(addr) = addr {
                        addresses.push(addr);
                    }
                    replies.push(json!({"verified":addr.is_some(), "line":line, "source":source(&actual),
                        "message":if unsupported { "conditional/log breakpoints are not supported" } else if addr.is_none() { "no code for this source line; build with -g" } else { "" }}));
                }
                self.source_bps.insert(file, addresses);
                self.rebuild_bps();
                Ok(json!({"breakpoints":replies}))
            }
            "setInstructionBreakpoints" => {
                let t = self.target()?;
                let limit = t.prog().len() as u64 / if matches!(t, Target::P16(_)) { 2 } else { 1 };
                let mut addresses = Vec::new();
                let mut replies = Vec::new();
                for bp in a["breakpoints"]
                    .as_array()
                    .ok_or("breakpoints must be an array")?
                {
                    let addr = address(&bp["instructionReference"])
                        .ok()
                        .and_then(|n| (n as i64).checked_add(bp["offset"].as_i64().unwrap_or(0)))
                        .filter(|n| *n >= 0 && (*n as u64) < limit)
                        .map(|n| n as u32);
                    let valid = addr.is_some_and(|n| matches!(t, Target::P16(_)) || n % 2 == 0)
                        && bp["condition"].is_null()
                        && bp["hitCondition"].is_null();
                    if valid {
                        addresses.push(addr.unwrap());
                    }
                    replies.push(json!({"verified":valid, "instructionReference":addr.map(|n| format!("0x{n:X}"))}));
                }
                self.instruction_bps = addresses;
                self.rebuild_bps();
                Ok(json!({"breakpoints":replies}))
            }
            "setExceptionBreakpoints" => Ok(json!({"breakpoints":[]})),
            "configurationDone" => {
                self.target()?;
                if self.configured {
                    return Err("already configured".into());
                }
                self.configured = true;
                Ok(json!({}))
            }
            "threads" => {
                self.target()?;
                Ok(json!({"threads":[{"id":1,"name":"PIC core"}]}))
            }
            "stackTrace" => {
                let t = self.stopped()?;
                let pcs: Vec<u32> = std::iter::once(t.pc()).chain(t.sites()).collect();
                let start = a["startFrame"].as_u64().unwrap_or(0) as usize;
                let count = a["levels"]
                    .as_u64()
                    .filter(|n| *n != 0)
                    .unwrap_or(pcs.len() as u64) as usize;
                let frames: Vec<Value> = pcs.iter().enumerate().skip(start).take(count).map(|(i, &pc)| {
                    let d = t.debug();
                    let mut f = json!({"id":i+1,"name":d.func_at(pc).map(|n| d.func_label(n)).unwrap_or(format!("0x{pc:X}")), "line":1,"column":1,"instructionPointerReference":format!("0x{pc:X}")});
                    if let Some((file, line)) = d.lines.lookup(pc) { f["source"] = source(file); f["line"] = json!(line); }
                    f
                }).collect();
                Ok(json!({"stackFrames":frames, "totalFrames":pcs.len()}))
            }
            "scopes" => {
                let t = self.stopped()?;
                let id = a["frameId"].as_u64().ok_or("frameId required")?;
                if id == 0 || id > 1 + t.sites().len() as u64 {
                    return Err("invalid frameId".into());
                }
                Ok(
                    json!({"scopes":[{"name":"Locals", "presentationHint":"locals", "variablesReference":id*2, "expensive":false},
                                     {"name":"Globals", "presentationHint":"globals", "variablesReference":id*2+1, "expensive":false}]}),
                )
            }
            "variables" => {
                let t = self.stopped()?;
                let reference = a["variablesReference"]
                    .as_u64()
                    .ok_or("variablesReference required")?;
                let frame = reference / 2;
                if frame == 0 || frame > 1 + t.sites().len() as u64 {
                    return Err("invalid variablesReference".into());
                }
                let (d, frames) = (t.debug(), t.frames());
                let f = frames.get(frame as usize - 1);
                let mem = d.data_view(t.mem());
                let rom = d.rom_image(t.prog());
                let variables: Vec<Value> = d.vars.iter().filter(|v| if reference % 2 == 1 { v.scope.is_none() } else { v.scope.is_some() && v.scope == f.and_then(|f| f.func) }).map(|v| {
                    json!({"name":d.var_label(v), "value":v.show_framed(&mem, &rom, d, f.and_then(|f| f.base)), "type":v.ty, "variablesReference":0})
                }).collect();
                Ok(json!({"variables":variables}))
            }
            "setVariable" => {
                let t = self.stopped()?;
                let reference = a["variablesReference"].as_u64().ok_or("variablesReference required")?;
                let (v, base) = scope_var(t, reference, string(a, "name")?)?;
                let addr = t.data_place(&v, base)?;
                let new = v.layout.encode(string(a, "value")?, &t.peek(addr, v.bytes as usize))?;
                let t = self.target.as_mut().ok_or("launch a program first")?;
                t.write(addr, &new);
                let d = t.debug();
                Ok(json!({"value":v.show_framed(&d.data_view(t.mem()), &d.rom_image(t.prog()), d, base), "type":v.ty, "variablesReference":0}))
            }
            "dataBreakpointInfo" => {
                let t = self.stopped()?;
                let name = string(a, "name")?;
                let found = match a["variablesReference"].as_u64().filter(|r| *r > 0) {
                    Some(r) => scope_var(t, r, name),
                    None => t.debug().global(name, None).cloned().map(|v| (v, None)).ok_or_else(|| "unknown variable".to_string()),
                };
                let (v, base) = match found {
                    Ok(x) => x,
                    Err(e) => return Ok(json!({"dataId":Value::Null, "description":e})),
                };
                match t.data_place(&v, base) {
                    Ok(addr) => {
                        let mask = if let Form::Bit { bit } = v.layout.form { 1u32 << (bit & 7) } else { 0xFF };
                        Ok(json!({"dataId":format!("{addr:x}:{}:{mask:x}", v.bytes), "description":v.name, "accessTypes":["write"], "canPersist":false}))
                    }
                    Err(e) => Ok(json!({"dataId":Value::Null, "description":e})),
                }
            }
            "setDataBreakpoints" => {
                let t = self.stopped()?;
                let size = t.mem().len() as u32;
                let mut bytes: Vec<u32> = Vec::new();
                let mut bits: Vec<(u32, u8)> = Vec::new();
                let mut out = Vec::new();
                for b in a["breakpoints"].as_array().ok_or("breakpoints required")? {
                    let parsed = if b["accessType"].as_str().is_some_and(|t| t != "write") {
                        Err("only write access can be watched".to_string())
                    } else {
                        parse_data_id(b["dataId"].as_str().unwrap_or(""), size)
                    };
                    match parsed {
                        Ok((addr, len, mask)) => {
                            if mask == 0xFF {
                                for x in addr..addr + len {
                                    if !bytes.contains(&x) {
                                        bytes.push(x);
                                    }
                                }
                            } else if !bits.contains(&(addr, mask)) {
                                bits.push((addr, mask));
                            }
                            out.push(json!({"verified":true}));
                        }
                        Err(e) => out.push(json!({"verified":false, "message":e})),
                    }
                }
                let t = self.target.as_mut().ok_or("launch a program first")?;
                let d = t.debugger();
                d.watchpoints = bytes;
                d.bit_watch = bits;
                Ok(json!({"breakpoints":out}))
            }
            "evaluate" => {
                let t = self.stopped()?;
                let d = t.debug();
                let expr = string(a, "expression")?;
                if expr.eq_ignore_ascii_case("pc") {
                    return Ok(json!({"result":format!("0x{:X}",t.pc()), "variablesReference":0}));
                }
                if expr.eq_ignore_ascii_case("cycles") {
                    return Ok(json!({"result":t.cycles().to_string(), "variablesReference":0}));
                }
                let frame = a["frameId"].as_u64().unwrap_or(1);
                let frames = t.frames();
                if frame == 0 || frame > 1 + t.sites().len() as u64 {
                    return Err("invalid frameId".into());
                }
                let f = frames.get(frame as usize - 1);
                let v = d
                    .vars
                    .iter()
                    .find(|v| {
                        v.name.eq_ignore_ascii_case(expr)
                            && v.scope.is_some()
                            && v.scope == f.and_then(|f| f.func)
                    })
                    .or_else(|| d.global(expr, f.and_then(|f| f.func).map(|i| d.funcs[i].low)))
                    .ok_or("unknown variable (expressions are names, pc or cycles)")?;
                Ok(
                    json!({"result":v.show_framed(&d.data_view(t.mem()), &d.rom_image(t.prog()), d, f.and_then(|f| f.base)), "type":v.ty,"variablesReference":0}),
                )
            }
            "loadedSources" => {
                let t = self.target()?;
                Ok(
                    json!({"sources":t.debug().lines.source_files().iter().map(|p| source(p)).collect::<Vec<_>>()}),
                )
            }
            "continue" | "next" | "stepIn" | "stepOut" => {
                let t = self.stopped()?;
                if !self.configured {
                    return Err("configurationDone required".into());
                }
                self.running = Some(if command == "continue" {
                    Running::Continue {
                        skip: true,
                        depth: t.depth(),
                    }
                } else if command == "next"
                    && (a["granularity"] == "instruction" || t.debug().lines.is_empty())
                {
                    Running::Over { depth: t.depth() }
                } else if command == "stepIn"
                    && (a["granularity"] == "instruction" || t.debug().lines.is_empty())
                {
                    Running::Instruction
                } else {
                    Running::Line(t.begin(command == "next", command == "stepOut"))
                });
                Ok(json!({"allThreadsContinued":true}))
            }
            "pause" => {
                self.target()?;
                self.running = None;
                Ok(json!({}))
            }
            "restart" => {
                self.target.as_mut().ok_or("launch first")?.reset()?;
                self.running = None;
                Ok(json!({}))
            }
            "disconnect" | "terminate" => {
                self.running = None;
                Ok(json!({}))
            }
            _ => Err(format!("unsupported request: {command}")),
        }
    }
    fn advance(&mut self) -> Option<(&'static str, String)> {
        let run = self.running.as_mut()?;
        let t = self.target.as_mut()?;
        let result = match run {
            Running::Instruction => {
                let stop = t.step();
                Some(stop)
            }
            Running::Line(s) => t.line(s),
            Running::Over { depth } => {
                let mut result = None;
                for _ in 0..256 {
                    let before = t.depth();
                    if let Some(s) = t.step() {
                        result = Some(Some(s));
                        break;
                    }
                    if Some(t.pc()) == t.halt() && t.depth() <= before {
                        result = Some(Some(Stop::Halt));
                        break;
                    }
                    if t.sleep_dead() {
                        result = Some(Some(Stop::Sleep));
                        break;
                    }
                    if {
                        let pc = t.pc();
                        t.bps().contains(&pc)
                    } {
                        result = Some(Some(Stop::Breakpoint(t.pc())));
                        break;
                    }
                    if t.depth() <= *depth {
                        result = Some(None);
                        break;
                    }
                }
                result
            }
            Running::Continue { skip, depth } => {
                let mut result = None;
                for _ in 0..256 {
                    let pc = t.pc();
                    let d = t.depth();
                    let called = d > *depth;
                    *depth = d;
                    if Some(pc) == t.halt() && !called {
                        result = Some(Some(Stop::Halt));
                        break;
                    }
                    if !*skip && t.bps().contains(&pc) {
                        result = Some(Some(Stop::Breakpoint(pc)));
                        break;
                    }
                    *skip = false;
                    if let Some(stop) = t.step() {
                        result = Some(Some(stop));
                        break;
                    }
                    if t.sleep_dead() {
                        result = Some(Some(Stop::Sleep));
                        break;
                    }
                }
                result
            }
        };
        result.map(|stop| {
            self.running = None;
            match stop {
                None => ("step", String::new()),
                Some(Stop::Breakpoint(_)) => ("breakpoint", String::new()),
                Some(Stop::Watchpoint(_)) => ("data breakpoint", String::new()),
                Some(s) => (
                    "pause",
                    format!("{s:?}; PC 0x{:X}, cycles {}", t.pc(), t.cycles()),
                ),
            }
        })
    }
}
fn main() {
    if std::env::args().any(|s| s == "--help" || s == "-h") {
        println!("basil-sim-dap: Debug Adapter Protocol on stdin/stdout; see docs/DAP.md");
        return;
    }
    if let Err(e) = serve() {
        eprintln!("basil-sim-dap: {e}");
        std::process::exit(2);
    }
}
fn serve() -> Result<(), String> {
    let (tx, rx) = mpsc::sync_channel(32);
    std::thread::spawn(move || {
        let mut input = io::BufReader::new(io::stdin());
        loop {
            match read_message(&mut input) {
                Ok(Some(v)) => {
                    if tx.send(Ok(v)).is_err() {
                        break;
                    }
                }
                Ok(None) => break,
                Err(e) => {
                    let _ = tx.send(Err(e));
                    break;
                }
            }
        }
    });
    let mut wire = Wire {
        out: io::stdout().lock(),
        seq: 0,
    };
    let mut adapter = Adapter::default();
    loop {
        let message = if adapter.running.is_some() {
            rx.try_recv().map_err(|e| match e {
                mpsc::TryRecvError::Empty => mpsc::RecvTimeoutError::Timeout,
                _ => mpsc::RecvTimeoutError::Disconnected,
            })
        } else {
            rx.recv_timeout(Duration::from_secs(60))
        };
        match message {
            Ok(Ok(req)) => {
                if req["type"] != "request" {
                    continue;
                }
                let command = req["command"].as_str().unwrap_or("");
                let result = adapter.request(command, &req["arguments"]);
                let ok = result.is_ok();
                wire.response(&req, result).map_err(|e| e.to_string())?;
                if ok {
                    match command {
                        "launch" => wire.event("initialized", json!({})),
                        "configurationDone" if adapter.stop_on_entry => wire.event("stopped", json!({"reason":"entry", "threadId":1,"allThreadsStopped":true})),
                        "configurationDone" => { let depth = adapter.target()?.depth(); adapter.running = Some(Running::Continue { skip:false, depth }); Ok(()) },
                        "pause" | "restart" => wire.event("stopped", json!({"reason":if command == "restart" {"entry"} else {"pause"}, "threadId":1,"allThreadsStopped":true})),
                        "disconnect" | "terminate" => { wire.event("terminated", json!({})).map_err(|e| e.to_string())?; break; },
                        _ => Ok(()),
                    }.map_err(|e| e.to_string())?;
                }
            }
            Ok(Err(e)) => return Err(e),
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
        if let Some((reason, description)) = adapter.advance() {
            wire.event("stopped", json!({"reason":reason,"description":description,"threadId":1,"allThreadsStopped":true})).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn framing_counts_utf8_bytes_and_reads_consecutive_frames() {
        let mut wire = Wire {
            out: Vec::new(),
            seq: 0,
        };
        wire.event("output", json!({"output":"µ PIC"})).unwrap();
        wire.event("terminated", json!({})).unwrap();
        let mut input = io::Cursor::new(wire.out);
        assert_eq!(read_message(&mut input).unwrap().unwrap()["seq"], 1);
        assert_eq!(read_message(&mut input).unwrap().unwrap()["seq"], 2);
        assert!(read_message(&mut input).unwrap().is_none());
    }
    #[test]
    fn bad_frames_are_errors() {
        for s in [
            "Content-Length: -1\r\n\r\n",
            "Content-Length: 4194305\r\n\r\n",
            "Content-Length: 5\r\n\r\n{}",
            "X: y\r\n\r\n",
            "Content-Length: 2\r\nContent-Length: 2\r\n\r\n{}",
        ] {
            assert!(read_message(&mut io::Cursor::new(s)).is_err());
        }
    }
}
