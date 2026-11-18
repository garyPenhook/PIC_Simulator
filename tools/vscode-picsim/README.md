# Basil PIC Simulator Debugger

A local VS Code launcher for `basil-sim-dap`. Build the simulator with
`cargo build --release --workspace`, then set `basilSim.adapterPath` to the
absolute path of `target/release/basil-sim-dap`.

Build a VSIX with `python3 scripts/package-vscode.py` in the simulator workspace.
Install the resulting file through VS Code's **Extensions: Install from VSIX**.
Alternatively, open this directory in an Extension Development Host.

Compile firmware with `basil -g -O0 program.bas`; choose **Basil PIC Simulator**
when creating a launch configuration and set `program` and `device`.
Source breakpoints, step into/over/out, the call stack, typed variables,
restart, and pause/continue use the simulator's existing debug information.
The adapter also accepts XC8 ELF debug information. See `docs/DAP.md` in the
simulator repository for supported requests and limitations.
