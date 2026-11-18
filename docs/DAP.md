# Editor debugging with basil-sim-dap

Build with `cargo build --release --workspace`. `target/release/basil-sim-dap`
speaks the [Debug Adapter Protocol](https://microsoft.github.io/debug-adapter-protocol/specification)
over standard input/output using Content-Length framed UTF-8 JSON. Diagnostics go
to stderr. No TCP listener or external debugger is required.

## VS Code

Run `python3 scripts/package-vscode.py` to create
`target/basil-picsim-debug-0.1.0.vsix`, then use **Extensions: Install from VSIX**.
Set `basilSim.adapterPath` to the absolute path of `basil-sim-dap` in Settings.
The extension source is `tools/vscode-picsim`; it has no npm dependencies.

Compile firmware with debug information (`basil -g -O0 main.bas`). In the firmware
workspace, add `.vscode/launch.json`:

```json
{
  "version": "0.2.0",
  "configurations": [{
    "name": "Debug PIC firmware",
    "type": "basil-sim",
    "request": "launch",
    "program": "${workspaceFolder}/dist/main.hex",
    "device": "18F27Q10",
    "stopOnEntry": true
  }]
}
```

Use the actual HEX/ELF path produced by your build. An ELF loads its adjacent HEX,
or `dist/<stem>.hex`. Debug information loads from the ELF beside the HEX or one
folder above it. An optional `asm` names an assembly listing for additional
symbols; otherwise `<hex-stem>.S` is read when present. `stimulus` names a TOML
stimulus file. `devices` overrides the device-description directory, and `cwd`
resolves relative launch paths. Without `device`, a device-tagged ELF is required.

Launch stops at reset after configuration by default. Continue runs to a source
or instruction breakpoint, `__halt`, or a Sleep with no wake source. At `__halt`
the session stays stopped so you can inspect variables or restart. A firmware
`Terminate` loop remains a running loop if the compiler emits it separately from
`__halt`; Pause stops it. Run control yields every 256 instructions so Pause and
Disconnect remain responsive even for endless firmware loops.

## Supported requests

* Initialization, launch, configurationDone, threads, disconnect and terminate.
* Source breakpoints (exact executable lines; replacement per source file),
  instruction breakpoints (replacement as a set), loadedSources.
* Continue, pause, stepIn, next, stepOut, restart; instruction granularity for
  stepIn and next. With no source table these two fall back to instruction stepping.
* Stack traces and scopes for live hardware-stack frames; globals, locals and
  parameters use the simulator's typed DWARF formatting and software-frame bases.
* Evaluate a variable name, `pc` or `cycles`. Hover evaluation has no side effects.

Addresses use native PC units: bytes on PIC18, words on PIC16. Lines and columns
are one-based and source paths are native filesystem paths. Optimized code can
merge lines or inline calls; build with `-O0` to retain routine boundaries.
Arrays and structures are formatted values rather than expandable variable trees.

Conditional/hit/log breakpoints, arbitrary expressions, memory editing, data
breakpoints, disassembly requests and reverse execution are not advertised.
Unsupported requests return failed responses; unsupported source breakpoints are
unverified. The core still supports the GUI's editing/watchpoint/VCD features.

## Validation

`cargo test -p picsim-cli --bin basil-sim-dap` checks framing and malformed input.
`python3 tests/dap/run.py` compiles real `-g -O0` firmware and exercises complete
PIC18 and PIC16 sessions, including breakpoints, stack frames, variables,
step into/out, halt inspection, restart and pausing endless firmware. It needs
the sibling compiler's release binary; `--basil` and `--adapter` override paths.

`tests/vscode/smoke.js` also exercises the launcher in the actual VS Code
extension host on both cores. See `tests/vscode/README.md` for the isolated
editor invocation. This covers extension activation and editor-managed launch,
breakpoints, stack/variables, stepping, restart and disconnect.
