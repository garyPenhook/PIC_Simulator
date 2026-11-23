# PicSim 0.2.16

PIC16/PIC18 firmware simulation with a graphical debugger, command-line runner,
programmable stimulus and editor debugging through the Debug Adapter Protocol.

## Changes

* Source-level debugging of programs from a PIC BASIC compiler that writes a COFF
  file (`prog.cof` beside the HEX, for example with `Declare Create_Coff = On`):
  line table and variables from the COFF file, routines, parameters and declared
  types from the BASIC source, the Microchip floating-point format, names in any
  case, and the part from the build log (`prog.pbe`) or listing. `Bit` variables
  show as 0 or 1 and the main program appears in the call stack as `Main`.
* The `pic-sim` window loads that COFF file the same way and takes the part from
  the build log. The VS Code extension launches such a HEX with no Basil build and
  no `device`.
* Debug adapter: `setVariable` (scalars, enumerations, pointers, floats and `Bit`
  variables; a `Bit` changes only its bit; a value that does not fit is refused)
  and data breakpoints (`dataBreakpointInfo`, `setDataBreakpoints`; a `Bit`
  watches only its own bit). The library has `Debugger::bit_watch` and
  `Ty::encode`.
* Oscillator: timed clock switching on PIC18 Q10/Q41, PIC16 (OSCCON1 parts) and
  K22 (oscillator start-up timer, PLL lock, HFINTOSC stabilisation).
* EUSART: synchronous master and slave mode.
* MSSP I2C client mode against a scripted external host (`[[i2c_host]]`): 7-bit
  and 10-bit addressing, general call, clock stretching, transmit and receive,
  the host's SCL/SDA waveform on the pins, an outside device holding SCL or SDA
  low; `[[i2c_host]]` is accepted on PIC16 at byte level.
* Manual and `docs/DAP.md` updated and the manual rebuilt.

## Downloads and installation

`picsim-0.2.16-linux-x86_64.tar.gz` contains `pic-sim`, `basil-sim`,
`basil-sim-dap`, device descriptions, documentation and a blink example.
Extract it and run the binaries in place; keep `devices/` beside them.
This binary bundle targets x86-64 Linux and was built on Kali Linux with its
system libraries; the GUI needs a working graphical desktop.

`PicSim_Manual.pdf` is also available separately. Install
`basil-picsim-debug-0.1.1.vsix` with VS Code's **Extensions: Install from VSIX**,
then set `basilSim.adapterPath` to `basil-sim-dap`. See `docs/DAP.md` for launch
configuration and supported requests. `SHA256SUMS` covers these downloads.

From source, build with `cargo build --release --workspace` and run
`python3 scripts/install-local.py` to install the three binaries, devices and
manual under `~/.local` (or pass `--prefix`).

## Validation and limits

704 workspace tests pass. Two tests in `crates/picsim/tests/wrong_part.rs` fail
in this tree and also fail without the changes of this release; they have not
been investigated. COFF debugging was checked with PIC18F27Q10 and PIC16F18313
programs; the PIC16F17146 was rejected by the compiler and is untested. The
`pic-sim` window and the VS Code launch of a COFF build were built but not
exercised by hand, and `setVariable` on a PIC16 software-stack variable that
crosses a memory bank is untested. A `Bit` set on a port register writes back
the byte as it reads. The adapter replaces its whole data-breakpoint set on each
`setDataBreakpoints`.

Peripheral timing assumptions and remaining unsupported modes are documented in
`docs/FACTS-SIM.md`; these results do not establish complete silicon timing
agreement.
