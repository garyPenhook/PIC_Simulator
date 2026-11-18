# PicSim 0.2.13

PIC16/PIC18 firmware simulation with a graphical debugger, command-line runner,
programmable stimulus and editor debugging through the Debug Adapter Protocol.

## Changes

* Peripherals tab: a pending interrupt flag whose enable is set now says
  whether the global enables (`GIE`, `PEIE`, `IPEN`, `GIEH`, `GIEL`) let it
  interrupt, names the blockers, and shows the priority. Rules follow the
  Q10, Q41, K22 and PIC16 data sheets.
* Right-click a pin in the Pins tab, or a register in the SFRs tab, to add it
  to the Waves capture.
* Waves plot is thinned above about 4,000 visible samples for speed; zoom in
  for detail. VCD export is unchanged.
* Earlier in the 0.2.12 line: Pins tab, change highlighting, Peripherals tab
  and an interactive Waves tab.
* New `Debugger::add_signal`; manual and release notes updated.

## Downloads and installation

`picsim-0.2.13-linux-x86_64.tar.gz` contains `pic-sim`, `basil-sim`,
`basil-sim-dap`, device descriptions, documentation and a blink example.
Extract it and run the binaries in place; keep `devices/` beside them.
This binary bundle targets x86-64 Linux and was built on Kali Linux 2026.3 with
its system libraries; the GUI needs a working graphical desktop.

`PicSim_Manual.pdf` is also available separately. Install
`basil-picsim-debug-0.1.0.vsix` with VS Code's **Extensions: Install from VSIX**,
then set `basilSim.adapterPath` to `basil-sim-dap`. See `docs/DAP.md` for launch
configuration and supported requests. `SHA256SUMS` covers these downloads.

From source, build with `cargo build --release --workspace` and run
`python3 scripts/install-local.py` to install the three binaries, devices and
manual under `~/.local` (or pass `--prefix`).

After building, `python3 scripts/package-release.py` creates the Linux archive,
manual, VSIX and checksums under `target/dist/0.2.13/`.

## Validation and limits

624 workspace tests and 694 selected compiler program runs pass; stimulus and
real-DWARF DAP sessions also pass. Q10 MDB comparisons agree on 407 of 442
runs; 35 cycle-count differences are retained in the regression report,
including a confirmed MDB computed-PCL timing discrepancy. SFR audits have
zero new differences across Q10/Q41/25K22/45K22.

Peripheral timing assumptions and remaining unsupported modes are documented
in `docs/FACTS-SIM.md`; these results do not establish complete silicon timing
agreement. The VS Code launcher is packaged and tested in actual VS Code
extension-host sessions on PIC16 and PIC18; manual UI inspection remains open.
