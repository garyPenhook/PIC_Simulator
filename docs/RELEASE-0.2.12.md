# PicSim 0.2.12

PIC16/PIC18 firmware simulation with a graphical debugger, command-line runner,
programmable stimulus and editor debugging through the Debug Adapter Protocol.

## Changes

* `pic-sim` Pins tab: level and direction of every pin at the current cycle
  (driven high/low, input high/low, analog); click an input to drive it.
* Orange change highlighting in the Memory and SFRs tabs (with a
  changed-only filter) and on pins, against the state when the last Step or
  Run began.
* Peripherals tab: pending interrupt flags with their enables, timer counts
  with period and control registers, and EUSART and ADC registers, decoded
  from the SFRs.
* Waves tab: interactive capture plot with time axis, zoom, pan and a hover
  readout; VCD export is unchanged.
* New `Periph::pin_overview` / `Pic16::pin_overview` with tests.
* Manual updated for the new tabs.

## Downloads and installation

`picsim-0.2.12-linux-x86_64.tar.gz` contains `pic-sim`, `basil-sim`,
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
manual, VSIX and checksums under `target/dist/0.2.12/`.

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
