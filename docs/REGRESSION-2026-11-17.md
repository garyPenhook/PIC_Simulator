# Regression baseline — 2026-11-17

Simulator base `6867ce3` plus this change; compiler `800ec26`, Basil 0.1.260;
Rust 1.99.1; MPLAB X MDB 6.35. Machine-readable results and explicit exclusions
are in [REGRESSION-2026-11-17.json](REGRESSION-2026-11-17.json).

## Compiler execution matrix

`python3 scripts/regression.py` runs the sibling compiler suites with the release
simulator. All **694 selected runs pass**, including default optimization and
`-O0`. These are declared result-variable/cycle-bound checks, not full GPR-snapshot
comparisons. Device restrictions in test comments still apply.

| Device | Suite | Default passed | -O0 passed |
|---|---|---:|---:|
| 18F27Q10 | exec | 229 | 229 |
| 18F16Q41 | exec_q41 | 13 | 13 |
| 18F25K22 | exec_k22 | 17 | 17 |
| 18F45K22 | exec_k22 | 17 | 17 |
| 16F17146 | exec_p16 | 33 | 33 |
| 16F13145 | exec_p13 | 4 | 4 |
| 16F13245 | exec_p132 | 5 | 5 |
| 16F15244 | exec_p152 | 9 | 9 |
| 16F18056 | exec_p180 | 9 | 9 |
| 16F18346 | exec_p183 | 11 | 11 |

The initial run found a real description defect: PIC16F18056/76 PCON0 bit 5 was
writable despite being unimplemented in DS40002325C 10.12.2 p.76. Mask DFh fixes
`exec_p180/lib_system` at both optimization levels.

Reviewed fixture exclusions remain visible in the runner's JSON output:

* Q41 `lib_dma`, `lib_dma_eeprom`, `lib_dma_flash` expect MDB's absent DMA engine
  to leave DGO set. Native DMA now progresses/completes. Four `dma:` programs
  require the compiler's older MDB replay/mailbox mechanism. Engine tests and
  `tests/sim/dma_q41.bas` validate actual native transfers instead.
* K22 `lib_spi` and `lib_pwm_out` hardcode 28-pin MSSP2/CCP4 wiring, so are excluded
  only on 18F45K22. Both pass on 18F25K22; existing simulator tests cover fixed
  40-pin routing.
* `select_dense_isr` explicitly requests MDB and is excluded from simulator-only
  runs. Source `devices:` restrictions skip two 40-pin-only K22 fixtures on the
  28-pin part and three alternate-package PIC16F132xx fixtures on 13245.

## MDB differential

The optimized Q10 run compared 221 eligible programs: **201 agree and 20 have
cycle-count differences**. All 221 independently pass the simulator's declared
results. The compiler's `--engine both` reports cycle mismatches before checking
all returned values, so agreement of both engines' results is not claimed for
those 20. The command deliberately exits 1; differences are not silently allowed.

| Program | Simulator cycles | MDB cycles |
|---|---:|---:|
| bit_packed_isr | 101564 | 101550 |
| isr_dual | 120635 | 120634 |
| isr_single | 159781 | 160074 |
| lib_adc | 717 | 633 |
| lib_adin | 1345 | 1177 |
| lib_aht30 | 84522 | 3920648 |
| lib_eeprom | 480370 | 373 |
| lib_i2c | 10242 | 9600968 |
| lib_intosc | 1000 | 3600970 |
| lib_intosc_div | 47 | 1440053 |
| lib_pwm_pins | 118152 | 117624 |
| lib_rtcc | 2575 | 1920653 |
| lib_serial | 8429 | 15098 |
| lib_spi | 965 | 1980580 |
| lib_spi_pins | 10560 | 10432 |
| lib_system | 1616 | 1488 |
| select_dense | 2574 | 2543 |
| select_dense_far | 2900 | 2892 |
| select_dense_many | 14520 | 14371 |
| serial_word_digits | 21305 | 35761 |

The `-O0` Q10 comparison also completed: **206 of 221 agree; 15 have cycle-count
differences**. Across both optimization levels that is 407 matching runs and 35
reported differences, with no automatic allowlist. The unoptimized differences:

| Program | Simulator cycles | MDB cycles |
|---|---:|---:|
| bit_packed_isr | 102671 | 102658 |
| isr_dual | 143771 | 143769 |
| isr_single | 154287 | 154776 |
| lib_adc | 1903 | 1815 |
| lib_adin | 1448 | 1272 |
| lib_aht30 | 85855 | 4161951 |
| lib_eeprom | 480442 | 454 |
| lib_i2c | 15381 | 10206042 |
| lib_intosc | 1224 | 3601191 |
| lib_intosc_div | 49 | 1440055 |
| lib_rtcc | 4005 | 2042069 |
| lib_serial | 9567 | 16238 |
| lib_spi | 12371 | 1991990 |
| lib_system | 2047 | 1869 |
| serial_word_digits | 23497 | 37953 |

The computed-jump difference is confirmed separately: MOVLW 0; ADDWF PCL,F; NOP
costs 4 cycles in the simulator and 3 in MDB, with the sampled RAM agreeing.
DS40002043F 11.2.2 p.114 and instruction-summary note 3 p.630 require the flush
cycle; the simulator retains it (D-SIM-15). Reproduce:

```sh
DFP="$HOME/.mchp_packs/Microchip/PIC18F-Q_DFP/1.31.492/xc8" \
  python3 tools/probe.py /tmp/pcl-probe tests/probe/pcl_cycles.S 18F27Q10 'DUMMY=0'
```

The remaining differences include previously documented timer/interrupt races,
ADC timing, NVM write latency and serial/oscillator/bus behavior. They remain
explicit investigation items; this report does not classify every mismatch as
an MDB defect. Full GPR snapshots, every device variant and silicon validation
remain open acceptance work.

## Other validation

* `cargo test --workspace`: **624 tests pass**, no failures.
* `cargo build --release --workspace`: passes, including CLI, GUI and DAP adapter.

* `sh tests/sim/run.sh`: all 10 stimulus/compiler integration programs pass,
  including UART, ADC, SPI, I2C, LED timing and the new native Q41 DMA transfer.
* `python3 tests/dap/run.py --adapter target/release/basil-sim-dap`: PIC18 and
  PIC16 sessions pass with actual `-g -O0` DWARF (source breakpoints, stack,
  typed variables, step into/over/out, instruction breakpoints/steps, halt, restart, pause/disconnect).
* `tools/audit/run.sh`: zero new SFR differences for Q10, Q41, 25K22 and 45K22.
* VS Code extension JavaScript syntax, VSIX ZIP/XML structure and Python scripts
  validate locally. Automated sessions in the actual VS Code extension host pass
  on PIC18 and PIC16: launch, source breakpoints, stack frames, variables,
  stepping into/out/over, restart and disconnect. This caught and fixed the
  launcher's activation event (`onDebugResolve:basil-sim`). Manual UI inspection
  remains separate; see `tests/vscode/README.md` for reproduction.
* Installation into an isolated prefix succeeds with all three release binaries,
  128 device descriptions and four documents; the installed DAP adapter passes
  the real-DWARF integration sessions.
* The manual PDF is rebuilt from the updated Markdown.

The scanner, additional PWM modes and K22 watchdog/IOC follow cited datasheets
with explicit timing assumptions in A156–A159; their engine tests pass. These
changes do not claim exhaustive hardware or electrical fidelity.
