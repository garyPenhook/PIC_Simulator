# PicSim 0.2.17

PIC16/PIC18 firmware simulation with a graphical debugger, command-line runner,
programmable stimulus and editor debugging through the Debug Adapter Protocol.

## Changes

* A `.cof` file can be opened directly. The `pic-sim` Browse dialog, drag and drop,
  `basil-sim` and the VS Code `program` setting accept a COFF file from a PIC BASIC
  compiler as well as `.hex` and `.elf`; the HEX written with it (beside it, or in
  `dist/`) is loaded and the COFF file gives the source lines and variables.
  Before, only a HEX with a `.cof` beside it could be chosen.
* The debug adapter finds the HEX of an `.elf` or `.cof` program the same way as
  the other tools, and the `.elf` extension is matched in any case.
* VS Code extension 0.1.2 (description of `program` updated). Manual rebuilt.

See `docs/RELEASE-0.2.16.md` for the COFF debugging, `setVariable`, data
breakpoint, oscillator, EUSART and I2C changes that this release builds on.

## Downloads and installation

`picsim-0.2.17-linux-x86_64.tar.gz` contains `pic-sim`, `basil-sim`,
`basil-sim-dap`, device descriptions, documentation and a blink example.
Extract it and run the binaries in place; keep `devices/` beside them.
This binary bundle targets x86-64 Linux and was built on Kali Linux with its
system libraries; the GUI needs a working graphical desktop.

`PicSim_Manual.pdf` is also available separately. Install
`basil-picsim-debug-0.1.2.vsix` with VS Code's **Extensions: Install from VSIX**,
then set `basilSim.adapterPath` to `basil-sim-dap`. See `docs/DAP.md`.
`SHA256SUMS` covers these downloads.

## Validation and limits

The workspace tests pass except two in `crates/picsim/tests/wrong_part.rs`, which
also fail without the changes of this release and have not been investigated.
Opening a `.cof` was checked with the command-line runner on the PIC18 test
program; the file dialog, drag and drop and the VS Code launch were built but not
exercised by hand.
