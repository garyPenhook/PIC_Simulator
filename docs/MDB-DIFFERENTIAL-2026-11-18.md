# MDB differential classification — 2026-11-18

Base `3183408`. Tool: `python3 scripts/gpr_snapshot.py --device 18F27Q10 [--flags=-O0] --json OUT`, which runs each
`tests/exec` program to `__halt` in basil-sim and MDB 6.35 and compares **every GPR byte** of the device file (3615 bytes)
and the cycle count. Machine-readable results: `target/regression/gpr-Q10-O.json`, `gpr-Q10-O0.json` (not committed).

## Snapshot result (221 programs, 9 not selected: `devices:`/`engine:`/`stim:`/`dma:` lines)

| Build | Identical GPR snapshots | Differ |
|---|---:|---:|
| default | 214 | 7 |
| `-O0` | 212 | 9 |

Programs with a different snapshot: `isr_dual`, `lib_aht30`, `lib_i2c`, `lib_intosc`, `lib_intosc_div`, `lib_rtcc`, `lib_spi`,
`lib_timer`, plus `lib_system` at `-O0`. Everything else, including every program that has only a cycle difference, has
identical GPR contents, so a cycle difference alone did not change any stored byte.

## The 20 cycle differences (default build) and their causes

| Program | Sim / MDB cycles | Cause | Status |
|---|---|---|---|
| select_dense, _far, _many | 2574/2543, 2900/2892, 14520/14371 | computed `addwf PCL` flush cycle, D-SIM-15 | MDB defect, data sheet cited |
| lib_spi_pins | 10560/10432 | same, +1 per computed jump (128 jumps) | D-SIM-15 |
| lib_pwm_pins | 118152/117624 | same; `addwf PCL` runs exactly 528 times | D-SIM-15 |
| isr_single | 159781/160074 | Timer0 prescaler lost around TMR0 accesses in MDB | D-SIM-1 |
| bit_packed_isr, isr_dual | 101564/101550, 120635/120634 | interrupt entry racing `bcf INTCON,7` | D-SIM-2, **fixed** (same day): now 101550/101550 and 120634/120634, `__ISV1` equal; entry by entry 200/200 and 320/320 equal to MDB |
| lib_serial, serial_word_digits | 8429/15098, 21305/35761 | MDB character time ≈ 18 bit times, data sheet 10 | D-SIM-17 (new) |
| lib_adc, lib_adin | 717/633, 1345/1177 | one ADC conversion 42 cycles shorter in MDB | D-SIM-18: MDB counts `ADACQ` in cycles and converts in about 13.5 TAD; the sim follows the register text; data sheet inconsistent (Figure 33-2 against 33.7.10), silicon not measured |
| lib_system | 1616/1488 | MDB `SysClockHz` returns 0 | D-SIM-19 (new) |
| lib_eeprom | 480370 / 373 (MDB finishes the write at once) | MDB does not model write durations | D-SIM-3 |
| lib_spi, lib_i2c, lib_aht30, lib_rtcc | up to 9.6 M in MDB | library timeouts expire in MDB; `SpiStatus` 1 there | MDB does not complete the MSSP transfer |
| lib_intosc, lib_intosc_div | 1000/3600970, 47/1440053 | oscillator-ready bits never set in MDB | D-SIM-14 |

`lib_timer` is the one program with **equal cycles and a different byte** (`LT_A` 2 against 3, 4 against 5 at `-O0`);
D-SIM-16 explains it: MDB keeps a Timer0 prescaler count across the `CLRF T0CON0`/`T0CON1`/`TMR0L` writes that DS40002043F
19.2.4 p.297 says clear it. Probe: `tests/probe/t0_read_phase.S`.

`gpr_snapshot.py` also reports `mdb-timeout` when MDB's own 300000-unit wait expires before `__halt`; none of the programs above
did (MDB reached `__halt` for the library-timeout cases, which is why their counts are millions, not a wait artifact). That check
was added after these runs; re-running the Q10 default build gave the same results for `lib_spi`, `lib_timer` and `abs_idiom`.

## What is not settled

* D-SIM-2 (interrupt entry race): the PIC18 core now runs the already-fetched instruction before an entry (`FACTS-SIM.md`). Left: 3 of 64
  single-shot phases of the `bcf INTCON,7` race with equal cycles and a different stacked address; `isr_single` entry cycles match for the first 54
  entries and then drift with D-SIM-1; the K22 `isr_dual` has a constant +1 entry offset (and now a different `__ISV1`, so 10 of 11 K22 programs are identical).
* D-SIM-18 (ADC conversion time on the Q10): compared with DS40002043F (`FACTS-SIM.md`). The sim is kept; only silicon can settle ADACQ's unit.
* Causes of D-SIM-16/-17/-19 inside MDB are not known; the entries record only what was measured.

## Other parts (default build only; `-O0` not run)

Same tool, one run per part. `gpr-<part>-O.json` in `target/regression`. The `skipped` count is programs the tool does not
compare (`devices:`/`engine:`/`stim:`/`dma:` lines, compile failures, C programs without a `.S`).

| Part | Identical | Differ | Differing programs |
|---|---:|---:|---|
| 18F27Q10 | 213 | 8 | see above |
| 18F16Q41 | 11 | 5 | lib_dma, lib_dma_eeprom, lib_dma_flash, lib_flashwrite, lib_timer |
| 18F25K22 | 11 | 0 | |
| 18F45K22 | 13 | 0 | |
| 16F17146 | 29 | 4 | lib_i2c, lib_intosc, lib_pwm, lib_spi |
| 16F13145 | 4 | 0 | |
| 16F13245 | 5 | 0 | |
| 16F15244 | 7 | 2 | lib_i2c, lib_spi |
| 16F18056 | 7 | 2 | lib_i2c, lib_spi |
| 16F18346 | 9 | 2 | lib_i2c, lib_spi |

* Q41 `lib_dma`, `lib_dma_eeprom`, `lib_dma_flash`: MDB has no DMA engine (`LD_AGAIN`, `LDE_D`, `LDF_D` are what DMA writes), the
  known case in `REGRESSION-2026-11-17.md`; the sim's values are the DMA results. `lib_flashwrite`: MDB 871 cycles against 516471,
  different bytes read back from flash, D-SIM-3/D-SIM-4 (MDB does not model flash self-write). `lib_timer`: D-SIM-16 again
  (`LT_A` 2 against 3, equal cycles), so that defect is not Q10-specific.
* PIC16 `lib_i2c` and `lib_spi`: MDB runs 3.2-16.8 million cycles and `SpiStatus` is 1 (timeout) there, 0 in the sim: the same
  MSSP-never-completes behaviour as the Q10 (lib_spi/lib_i2c). `lib_intosc` on the 17146: same oscillator-ready wait (D-SIM-14 is
  measured on the Q10; the 17146 timeout is consistent with it, not separately checked).
* PIC16F17146 `lib_pwm`: MDB 2 222 740 cycles against 2730, one scratch byte differs (`__A1` 0xEA, the high byte of
  60000). Explained: `Basic_Compiler/lib/pwm.bas` `WaitLoad` polls `PWM1CON.LD` (bit 2) with a 60000-iteration cap, 37 cycles per
  iteration (60000 x 37 = 2 220 000 of the 2 220 010 extra cycles). MDB never clears `LD`: read in MDB at entry to the second
  `PWM_LOADDUTY`, `PWM1CON` = 0x84 (EN and LD still set from the first load). The sim clears `LD` at the period reload
  (FACTS-SIM.md, PWM load), as the data sheet describes. MDB does not model the PWM reload, so the sim is right and the library is
  unchanged; `lib_pwm` is a known MDB gap (same class as `CCPxCON.OUT`, REVIEW-FINDINGS T3).
* The K22 parts and the 16F13145/13245 parts have identical snapshots on every program the tool compares. The PIC16 exec
  suites are small (4-33 programs per part), so that is narrower evidence than the Q10's 221 programs.
* Every PIC16 cycle-count agreement that the tool reports as `same` with a very different count (`lib_serial` 4235/463 on the 17146)
  shows MDB and the sim agreeing on RAM while disagreeing on timing, as on the Q10 (D-SIM-17).
