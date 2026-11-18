# PicSim 0.2.14

PIC16/PIC18 firmware simulation with a graphical debugger, command-line runner,
programmable stimulus and editor debugging through the Debug Adapter Protocol.

## Changes

* Peripherals tab: **Fire an interrupt**. Every interrupt flag of the part is
  listed with a **Fire** or **Clear** button. **Also enable it** sets the
  flag's enable bit and the global enables that block it (`PEIE`, `GIE`/`GIEH`,
  `GIEL`). The status line reports whether the interrupt will be taken, what
  blocks it, or that the flag is read-only on the part.
* Earlier in the 0.2.x line: interrupt gating in the Peripherals tab,
  right-click trace from the Pins and SFRs tabs, Pins tab, change highlighting
  and an interactive Waves tab.
* Manual rebuilt.

## Downloads and installation

`picsim-0.2.14-linux-x86_64.tar.gz` contains `pic-sim`, `basil-sim`,
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
manual, VSIX and checksums under `target/dist/0.2.14/`.

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
