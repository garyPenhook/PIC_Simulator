# Simulator plan (`basil-sim`)

Current status: 2026-11-17. This roadmap tracks the standalone simulator workspace.
Compiler execution suites and MDB replay tooling live in the sibling
`Basic_Compiler` checkout. The original compiler decisions D57/D58 established
Rust and instruction-cycle timing; `FACTS-SIM.md` now records the implemented
rules, datasheet citations, assumptions and known MDB differences.

## Implemented architecture

* `crates/devdesc`: shared TOML device descriptions with memory regions, SFR
  masks/reset values, configuration fields and module identifiers.
* `crates/picsim`: `Pic18` and `Pic16` cores, HEX/config/EEPROM loading,
  device-driven memory maps, interrupt controllers and peripheral models.
  `Machine`/`Machine16` provide execution, breakpoints, typed DWARF information,
  instruction/source stepping, watchpoints and waveform capture.
* Peripheral binding discovers registers by name and uses address dispatch.
  Lazy synchronization advances peripherals at instruction boundaries and
  scheduled events; active serial/PWM/DMA/CRC models retain their documented
  finer timing rules. There is no generic boxed `Periph` registry or `Core` trait.
* `tools/picsim`: `basil-sim` CLI and `basil-sim-dap` stdio debug adapter.
  `tools/picsim-gui`: `pic-sim` egui debugger and board view.
  `tools/vscode-picsim`: local VS Code DAP launcher, packaged by
  `scripts/package-vscode.py`; setup and protocol scope are in `DAP.md`.
* Unsupported SFR accesses produce diagnostics; `--strict` stops on them.
  Supported peripherals can still have explicitly diagnosed unsupported modes.

## Milestones

| Milestone | Current deliverable | Remaining acceptance work |
|---|---|---|
| S0: device data | Reset values, access metadata, masks and module IDs in descriptions; local SFR audit baselines | Keep audits and hand-maintained expectations aligned with supplied datasheets |
| S1: PIC18 core | HEX decoding, instruction timing, stack/context, CLI and debugger on Q10/Q41/K22 | Full GPR-snapshot differential gate across the compiler suite; known MDB differences must remain explicit |
| S2: PIC16 core | Enhanced mid-range families listed in `MANUAL.md`, including 5-bit BSR 183xx | Broaden differential coverage to all device variants and compiler programs |
| S3: interrupts/timers/config | Legacy and Q41 vectored interrupts, timers, configuration, reset/Sleep, watchdogs | Hardware measurements for documented assumptions and reset/timing differences |
| S4: pins/serial/ADC/stimulus | PIC16/PIC18 pin connectivity, asynchronous serial, ADC variants, periodic clocks and reactive clients | Remaining ADC modes/families and unsupported serial/electrical protocols |
| S5: storage/buses/DMA | EEPROM/flash, host buses, Q41 DMA arbitration and CRC scanner | Retire legacy compiler DMA replay tests after migrating their artificial mailbox/SFR feeds to native stimuli |
| S6: debugger/editor | GUI editing/watchpoints/VCD, DWARF source stepping; DAP adapter and VS Code launcher with editor-host smoke coverage | Manual UI inspection; broader DAP conveniences such as disassembly, data breakpoints and expandable variable trees |

## Recent completion

* PIC16 GPIO/PPS, IOC/INT, timer pins, EUSART pin loopback, CCP and standalone
  10-bit/16-bit PWM are implemented. They are no longer outstanding S4 work.
* PIC18 GPIO, pull-ups/open drain and serial/internal pin connectivity are
  implemented, including fixed K22 UART/MSSP pins.
* Q41 DMA has four banks, CPU/ISR/channel priorities, priority lock/one-way rules,
  peripheral requests, bus sharing, Sleep/Idle and reset behavior.
* Q41 CRC scanner now feeds flash/EEPROM into CRC, obeys SCANPR against CPU/DMA,
  and supports burst/concurrent operation with LFINTOSC, Timer0 and CLC triggers.
* Center and variable alignment extend the shared Q41/PIC16 16-bit PWM engine.
* K22 watchdog and fixed INT0–2/PORTB mismatch interrupts now include Sleep
  wake-up; K22 TO/PD flags correctly use RCON. PIC16F18056/76 PCON0 bit 5 is
  masked as unimplemented.
* DAP sessions are tested against actual compiler DWARF on PIC18 and PIC16.
* `pic-sim` monitoring: a Pins tab (level and direction of every pin, click an input to drive it), orange change
  highlighting in the Memory and SFRs tabs (and a changed-only filter), a Peripherals tab (timer counts, EUSART/ADC
  registers, pending interrupt flags decoded from the SFRs) and an interactive Waves tab (zoom, pan, hover readout)
  replacing the fixed preview. `Periph::pin_overview` / `Pic16::pin_overview` are the pin source (`tests/pin_overview.rs`).

## Regression workflow

1. `cargo test --workspace` and `cargo build --release --workspace`.
2. `sh tests/sim/run.sh` for compiler/stimulus integration, including native Q41 DMA.
3. `python3 scripts/regression.py` runs ten representative parts at default
   optimization and `-O0`. It retains logs and `summary.json` under
   `target/regression`. Reviewed incompatible fixtures and legacy DMA replay
   exclusions carry a reason in every report; new failures fail the command.
4. `python3 scripts/regression.py --engine both --device 18F27Q10` compares
   declared result variables and cycle counts with MDB. Explicit single-engine
   tests/stimuli and legacy DMA replay are excluded. This runner does **not**
   compare all GPR bytes, so it does not close the full-snapshot gate above.
5. `python3 tests/dap/run.py --adapter target/release/basil-sim-dap` exercises
   framed DAP sessions; `python3 scripts/package-vscode.py` builds a local VSIX.
6. `tools/audit/run.sh` checks SFR descriptions against supplied PDFs.

`REGRESSION-2026-11-17.md` records this baseline and its scope. MDB does not model
several implemented peripherals, so disagreement is investigated against the
datasheet rather than automatically treated as a simulator error.

## Remaining peripheral work

These are bounded follow-on tasks, not claims of complete silicon coverage:

* Q41 scanner: remaining trigger sources, exact memory-bus/Q-clock ordering,
  NVM-busy contention and silicon measurements of transfer/CRC timing.
* Shared 16-bit PWM: non-FOSC clocks, external reset/automatic load sources and
  hardware load-request exclusion/propagation delays.
* PIC16: additional ADC coverage and computation modes, physical MSSP SPI/I2C
  waveforms and external clients beyond the existing OLED interface.
* K22: CCP capture/compare, external Timer0 clock, remaining analog peripherals.
* Across families: synchronous serial modes, I2C stretching/arbitration/client
  modes, oscillator start-up/fail-safe behavior and electrical/analog behavior.

New models require a datasheet citation, explicit assumptions, meaningful
engine-level tests, and updates to the manual and facts before being advertised.
