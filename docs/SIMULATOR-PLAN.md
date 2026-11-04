# Simulator plan (`basil-sim`)

A simulator of the PIC parts Basil supports, to replace MDB as the test oracle and to give the editor
extensions a debugger. Decisions D57 and D58 in `DECISIONS.md`.

## Starting point

* MDB (MPLAB X) is the oracle today (`tests/backend.rs`, `scripts/exec-test.py`). It is slow and closed,
  has no DMA or I2C host model, and has known deviations (D34, the Q41 double vectoring in D56).
* `scripts/picsim.py` is a PIC18-only Python simulator that parses the `.S` text, with a Q41 I2C1 host model.
  `scripts/dma_sim.py` is a DMA model attached to a live MDB session.
* `devices/*.toml` had no SFR reset values or access masks. The pack has them on every `SFRDef`
  (`por`, `mclr`, `access`, `impl`), and `_modsrc` names the peripheral IP block
  (e.g. `DOS-01631_tmr_mid0_v1_8bit_rev2`). Schema 0.2 records them (milestone S0).

## Decisions

* **Rust** (D57): crate `crates/picsim`, CLI `tools/picsim`. Reuses `devdesc` directly; the suite runs
  thousands of programs twice.
* **Cycle exact** (D58) for the cores and every peripheral model from the first milestone that has it:
  instruction timing, pipeline effects on skips/branches/table reads, interrupt latency, timer prescaler and
  synchroniser delays. `' cycles:` bounds and the benchmark harness (D18) rely on it. Every timing rule is
  cited to the data sheet; where the data sheet is silent the rule is marked `[A#]` and measured against MDB.

## Architecture

```
                 ┌──────────── Machine ──────────────┐
 .hex + .sym ──► │  Core (trait)                     │
 devices/X.toml  │   ├─ Pic18Core   (Q10, Q41)       │
 stimulus file   │   └─ Pic16ECore  (enh. mid-range) │
                 │  DataBus: banks, access bank,     │
                 │   FSR/INDF, linear RAM, SFR map   │──► trace / RAM dump /
                 │  ProgMem + config + EEPROM/NVM    │    cycle count / VCD
                 │  Scheduler (cycle-ordered events) │
                 │  IntController (per family)       │
                 │  Peripherals: Vec<Box<dyn Periph>>│
                 └───────────────────────────────────┘
```

### Device data

`devdesc` 0.2 adds to each SFR the pack's raw `por`, `mclr`, `access` strings, the `implemented` mask and the
`module` (`_modsrc`). The strings are kept verbatim (one character per bit, MSB first); the simulator decodes
them, and the meaning of each letter is recorded with its source in `docs/FACTS-SIM.md` before it is relied on.
An SFR without a peripheral model is a latch honouring `implemented` and the read-only bits, and its first
access logs a warning, so no test passes silently on an unmodelled peripheral.

### Cores — `trait Core { fn reset(&mut self, kind); fn step(&mut self, bus: &mut Bus) -> Cycles; }`

Both decode the **hex** (what pic-as produced, not the assembly text), table driven, with a predecode cache
per program word.

* `Pic18Core`: base instruction set, the Q41 additions the emitter uses (`movffl`, ...), hardware stack depth
  from `memory.hw_stack`, shadow registers per `Target.hw_context` (D56), `TBLRD`/`TBLWT`.
* `Pic16ECore`: enhanced mid-range set, `BSR` banking, linear data memory through the FSRs, automatic context
  save, `MOVIW`/`MOVWI`.

Every flag, timing and edge-case rule goes into `docs/FACTS-SIM-<family>.md` with its data sheet cite.

### Bus and memory

Address decoding comes from the same TOML regions the compiler uses, so the two cannot disagree. SFR access
dispatches through an address table built at load time (latch, or hook into a peripheral for read side
effects and write-to-clear bits). Configuration words are read from the hex and handed to the models
(oscillator, watchdog, `MVECEN`, `LVP`).

### Peripherals

```rust
trait Periph {
    fn bind(dev: &Device, instance: &str) -> Option<Self> where Self: Sized; // finds its SFRs by name
    fn read(&mut self, reg: RegId, now: Cycle) -> u8;
    fn write(&mut self, reg: RegId, v: u8, now: Cycle, sched: &mut Scheduler);
    fn event(&mut self, ev: EvId, now: Cycle, sched: &mut Scheduler, irq: &mut IrqLines);
}
```

A registry maps module ids to model constructors; a part gets every model whose module it contains.
Models are event driven (no per-cycle tick) but schedule at exact cycles.

Order, by what `lib/` uses: timers 0/1/2, interrupts and ports → UART (TX capture, RX feed) → NVM/EEPROM and
flash write → ADC (fed values) → MSSP SPI/I2C, PPS, PWM/CCP → Q41 I2C host (ported from `picsim.py`) →
DMA (ported from `dma_sim.py`).

A `Stimulus` file (TOML) drives pins over time, UART RX bytes, ADC values, and I2C/SPI client models
(the models in `tests/exec_rtcc` and `tests/exec_pins` become clients).

### Interrupt controllers

One per family: PIC18 two-priority, Q41 with `MVECEN = 0` (D56; `MVECEN = 1` is `E-CONFIG`, so not needed
yet), PIC16 single vector with automatic context save. Vector addresses from `[vectors]`.

### Front ends

* CLI: `basil-sim prog.hex --device 18F16Q41 --until __halt --dump T_R1,T_R2 --cycles --trace --stim s.toml --vcd out.vcd`
  (symbols from the `.sym` pic-as writes).
* `exec-test.py --engine=sim|mdb`; `' expect:`, `' cycles:`, `' dma:` lines work unchanged.
* GUI: `tools/picsim-gui` (`basil-sim-gui`, egui): load a hex, Run/Pause/Step/Step over/Reset, breakpoints by
  clicking a disassembly line, registers, return stack, memory and SFR views, watch list, time at a given Fosc.
* GUI file choice: Browse hex/elf… and Stim… (file dialogs), or drag a `.hex`, `.elf` or stimulus `.toml` onto the window. An `.elf`
  loads the `.hex` beside it (or in `dist/`). The Board panel (LEDs, OLED) is available before a program is loaded and its wiring is kept.
* Debug information (`crates/picsim/src/debuginfo.rs`, `lines.rs`): build with `basil -g`; the simulator reads `<stem>.elf` beside the hex
  (or one folder up; basil writes the hex into `dist/`) with `gimli`. From the compiler's own DWARF unit (producer `basil`): routines
  with address ranges, `__halt` (an empty range, so `--until __halt` and the GUI's run-to-halt work without the `.S` file), global
  variables, and each routine's parameters and locals, with type (name, size, signedness, array length). From `.debug_line`: address ->
  BASIC line (`.bas` files only; generated assembly has no line). Routines and globals also become `Machine::syms`; the listing's symbols
  win. `basil-sim` loads the ELF the same way. Without `-g` only the hex and the `.S` symbols exist.
* GUI debugging: the Source pane highlights the BASIC line the PC is on (library files such as `i2c.bas` included), follows the PC, and a
  click sets a breakpoint at the line's lowest address. Buttons: Line (into calls), Line over, Out (run until the routine returns),
  plus Step and Step over for instructions (`Machine::step_line`). The Variables tab lists the globals and the parameters and locals of
  the routine the PC is in, typed (`Integer` signed, `Char`/`String` as text, `Float` as binary32, arrays), live while running; values
  that changed since the last Step or Run are orange. The call stack shows the routine and BASIC line of the PC and of every return
  address. The optimizer can merge lines, so some lines have no code; library routines the compiler inlines run at the caller's stack
  depth, so Line over cannot skip them.
* Later: a Debug Adapter Protocol server for `vscode-extension`.

## Verifying the simulator

1. **Differential against MDB**: every `tests/exec*` program on both engines, `-O` and `-O0`; compare the full
   GPR snapshot and the cycle count at `__halt`. Known MDB deviations go in a cited allowlist.
2. **Instruction conformance**: generated opcode × flag edge-case programs, checked against the data sheet's
   instruction tables (like the `scripts/gen-*-tests.py` generators).
3. **Peripheral unit tests** from the data sheets' timing examples; `test_dma_sim.py` and `test_i2chw.py` carry over.
4. **Gate**: the simulator becomes the default engine only when the whole suite agrees with MDB;
   `BASIL_MDB=1` stays available.

## Milestones

| # | Deliverable | Done when |
|---|---|---|
| S0 | devdesc 0.2 (reset values, access, implemented mask, module), devgen, regenerated devices | `check-devices.sh` passes; compiler output byte-identical |
| S1 | Crate skeleton, hex + sym loader, `Pic18Core` cycle exact, latch SFRs, halt detection, CLI dump | peripheral-free `tests/exec` programs match MDB (RAM and cycles) on 18F27Q10 and 18F16Q41 |
| S2 | `Pic16ECore` | same on 16F17146, 16F13145, 16F18346, 16F15244 |
| S3 | Interrupt controllers, timers, configuration words | `' cycles:` and ISR tests pass |
| S4 | Ports, PPS, UART, ADC, stimulus file | `serial`, `adc`, `exec_pins` pass |
| S5 | NVM/EEPROM, SPI, I2C, Q41 I2C host, DMA | `exec_rtcc`, DMA and I2C tests pass; `picsim.py`, `dma_sim.py` retired |
| S6 | Trace, VCD, breakpoints, DWARF stepping, DAP | stepping a `.bas` file in the editor |

## Adding a part later

1. `devgen gen` → `devices/<part>.toml`, plus the hand-written `.expect.toml` (existing procedure).
2. Known core family and every module already in the registry: no code. Run the differential suite with
   `--device <part>`.
3. New peripheral IP block: one `Periph` impl registered under its module id, plus a FACTS section.
4. New core family (baseline PIC10/12, dsPIC, ...): one `Core` impl and interrupt controller; bus, scheduler,
   peripherals and front ends are shared.
5. `basil-sim --coverage <part>` lists the part's modules that have no model.

## Status

* S0 done (devdesc 0.2, regenerated devices).
* S1 done for the core: `crates/picsim` (hex loader, bus, `Pic18`, disassembler), `basil-sim` CLI, `basil-sim-gui`,
  `exec-test.py --engine sim|mdb|both` (DMA programs are skipped under `sim` until S5). Results and the rules
  relied on are in `FACTS-SIM.md`.
* Open for S1's "done when": the `both` comparison of cycle counts with MDB.
* S3 mostly done: interrupt controllers (Q10 legacy, Q41 `MVECEN = 0` with hardware context), Timer0/1/2. Configuration words done (`config.rs`, FACTS-SIM.md). Open: reset values after MCLR/watchdog, two documented MDB differences (`FACTS-SIM.md`, D-SIM-1/2). Hardware probes in `tests/hw/` can
  settle them on a PIC18F27Q10 (PICkit 4 `ipecmd`, or the PIC-Programmer) when a programmer is at hand.
* S4 started: EUSART1/2 transmitter of the Q10 style (TXIF, TRMT, baud timing, byte capture; `lib_serial`, `lib_serial2` pass). EUSART receiver and the stimulus file (`[[uart]]`, `basil-sim --stim`) done; `tests/sim/run.sh` and `cargo test -p picsim` cover them. ADC of the Q10 (basic mode, `[[adc]]` stimulus) and FVRRDY done; `lib_adc` and `lib_adin` pass. Q41 UART (`periph/quart.rs`: buffers, TXMTIF, FIFO flags, parity/address modes, stimulus) done. Q41 ADC (12 bits, 13-bit ADPRE/ADACQ in Fosc clocks, 14 TAD + 2 TCY) done. S5 started: Q10 data EEPROM (`periph/nvm.rs`, `tests/nvm.rs`). `tests/exec/lib_eeprom` passes under `exec-test.py --engine sim` (30414 cycles; not yet compared with MDB). Q41 data EEPROM (`periph/nvmq.rs`) done, `exec_q41/lib_eeprom` passes (30439 cycles). Program flash self-write (word, sector/page, TBLWT holding registers) done for Q10 and Q41, `lib_flashwrite` passes on both (28046 and 34286 cycles). NVM `--engine both` run against MDB (FACTS-SIM.md D-SIM-3/4): results agree, cycle counts cannot (MDB has no write times). Q41 SPI host (`periph/spi.rs`, `tests/spi.rs`, `tests/sim/spi_q41`; byte-level client from `[[spi]]` in the stimulus file, assumptions A40-A44) done. Q10 MSSP (`periph/mssp.rs`, `tests/mssp.rs`, `tests/sim/spi_q10`, `i2c_q10`; SPI host and I2C host against `[[spi]]` / `[[i2c]]` clients, assumptions A45-A50) done. Q41 I2C host (`periph/i2cq.rs`, `tests/i2cq.rs`; host mode against `[[i2c]]` clients, assumptions A51-A60) done; no basil-sim program yet (the Basic I2C library for the Q41 is not in this repo). Q10 completion started: pin-output layer (RxyPPS sources, `pin_log`) and CCP1/CCP2 capture, compare and PWM on Timer1/Timer2 (`periph/ccp.rs`, `tests/ccp.rs`, assumptions A61-A63). The Q10 has no DMA (its `dma_nvm` module is the NVM block); DMA belongs to the Q41. Timers 3-6 done (same models as Timer1/Timer2; CCPs follow CCPTMRS). PWM3/PWM4 done (`periph/ccp.rs` `Pwm`, PPS codes 0x07/0x08, Timer2/4/6 via P3TSEL/P4TSEL, assumptions A64-A66, tests in `tests/ccp.rs`). CWG1 done (`periph/cwg.rs`, `tests/cwg.rs`, assumptions A73-A78). Comparators C1/C2 done (`periph/cmp.rs`, `tests/cmp.rs`, `[[analog]]` stimulus, assumptions A79-A82). CLC1-CLC8 done (`periph/clc.rs`, `tests/clc.rs`, assumptions A83-A87; timer, clock and serial inputs read low with a note). WWDT done with the general non-power-on reset (`periph/wdt.rs`, `Pic18::reset_cond` used by the RESET instruction too, `tests/wdt.rs`, assumptions A88-A93). SLEEP with wake-up (watchdog, enabled interrupt flags) and the peripherals' Sleep rules done (`Periph::sync_asleep`/`thaw`, `tests/sleep.rs`, assumptions A93-A94). CRC and memory scanner done (`periph/crc.rs`, `tests/crc.rs`, `Special::Crc`, assumptions A95-A98). Stack over/underflow resets done (`tests/stack.rs`, assumption A99). MCLR pin reset done (`[[mclr]]` stimulus, `tests/mclr.rs`, assumption A100). External interrupts INT0-INT2 and interrupt-on-change with wake-up from Sleep done (`tests/ioc.rs`, assumption A101). Timer0/1 on the clocks that run in Sleep (T0CKIPPS/T1CKIPPS pin, LFINTOSC, SOSC; awake and in Sleep, wake-up on the overflow or CCP compare; `tests/timer_sleep.rs`, assumption A102) done. Next for the Q10: nothing open.
