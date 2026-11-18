# PicSim 0.2.11

PIC16/PIC18 firmware simulation with a graphical debugger, command-line runner,
programmable stimulus and editor debugging through the Debug Adapter Protocol.

## Changes

* Add `basil-sim-dap` and a local VS Code extension: source/instruction
  breakpoints, stepping into/over/out, stack frames, typed variables,
  pause/continue and restart on PIC16 and PIC18.
* Model the Q41 CRC scanner with flash/EEPROM sources, CPU/DMA priority
  arbitration, burst/concurrent operation and selected triggers.
* Add center/variable alignment to shared Q41/PIC16 16-bit PWM models.
* Model K22 watchdogs and fixed INT0–2/PORTB mismatch interrupts, including
  Sleep wake-up, RCON flags and mismatch-latch handling.
* Correct the PIC16F18056/76 PCON0 implemented-bit mask.
* Add repeatable regression reporting, native DMA integration coverage,
  installation/extension packaging tools, and updated roadmap/manual/PDF.

## Downloads and installation

`picsim-0.2.11-linux-x86_64.tar.gz` contains `pic-sim`, `basil-sim`,
`basil-sim-dap`, device descriptions, documentation and a blink example.
Extract it and run the binaries in place; keep `devices/` beside them.
This binary bundle targets x86-64 Linux and was built on Ubuntu 24.04 with
its system libraries; the GUI needs a working graphical desktop.

`PicSim_Manual.pdf` is also available separately. Install
`basil-picsim-debug-0.1.0.vsix` with VS Code's **Extensions: Install from VSIX**,
then set `basilSim.adapterPath` to `basil-sim-dap`. See `docs/DAP.md` for launch
configuration and supported requests. `SHA256SUMS` covers these downloads.

From source, build with `cargo build --release --workspace` and run
`python3 scripts/install-local.py` to install the three binaries, devices and
manual under `~/.local` (or pass `--prefix`).

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
