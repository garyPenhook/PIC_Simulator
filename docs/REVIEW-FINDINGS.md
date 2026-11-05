# Review findings (2026-11-05)

Review of the picsim device descriptions and test/tooling around the `lib_pwm` / `lib_dma_abort` disagreement between basil-sim and
MDB. Method: `tools/audit/audit_sfr.py` extracts every 8-bit register table (Access row = implemented bits, Reset row) from the data
sheet text and compares it with `devices/*.toml` by address. Data sheets: DS40002214G (Q41), DS40002043F (Q10), DS40002519D,
DS40002195E, DS40002343F (16F). MDB is not treated as a reference (it has known gaps).

Confidence: **C** = I read the register's page and the text confirms it. **P** = reported by the script only; read the page before
fixing (generic pages describe several instances with one table, and the Q10 data sheet covers the 28-pin and the 40-pin part).

Already fixed in `dfc1021` and in the Basic_Compiler tests: Q41 `PIR2`/`PIE2`/`IPR2`, `PIR0`/`PIE0`/`IPR0`, `PMD0`-`PMD4`, `WDTPSL/H`,
`DAC1DATL`/`DAC2DATL`, `MD1CARH`, `CLCDATA`; Q10 `PMD0`-`PMD5`, `PIE3`, `IPR3`; 16F `WDTPSL/H`, `PORTC`; the `OUT`-bit masking in `lib_pwm`.

## To fix: device descriptions

Priority H changes what a program can do or see in the simulator; M is a wrong mask or reset that is rarely reached; L is a mask
that is too wide (writes stick that should not).

| ID | Pri | Conf | Part | Register | Device file | Data sheet | Effect |
|---|---|---|---|---|---|---|---|
| R1 | H | C (IPR0) / P (rest) | Q41 (6 files) | `IPR0`-`IPR10` reset | 0x00 (with the dashes of the mask) | IPR0 `10111111`, IPR1/3/4/9 `11111111`, IPR2 `1111---1`, IPR5 `1111-111`, IPR6 `111111-1`, IPR7 `111-1111`, IPR8 `1-1-1111`, IPR10 `11111--1` | After reset every source is low priority in the model; the part resets to high priority. The Q10 IPRs are already correct. **Fixed:** all six Q41 device files now carry the data sheet resets (checked 2026-11-05). |
| R2 | H | C | Q10 | `DAC1CON0` (0xF2E) mask | 28 | 189 (EN 7, OE1 5, OE2 4, PSS 3:2, NSS 0) | `EN`, `OE1` and `NSS` cannot be set. **Fixed:** mask 189 in `18F27Q10.toml` (checked 2026-11-05). |
| R3 | M | C | Q10 | `STKPTR` (0xFFC) mask | 63 | 31 (STKPTR[4:0]) | Bit 5 writable. Fixed (mask 31). |
| R4 | M | C | Q10 | `OSCCON3` (0xED5) reset | `00-00---` | `01-00---` (SOSCPWR = 1) | Wrong power-on value. Fixed. |
| R5 | M | C | Q41 | `PCON1` (0x4F1) reset | `-----111` | `-----10q` (MEMV = 0 on POR/BOR) | Wrong power-on value of MEMV. Fixed (`-----10q`). |
| R6 | M | C | Q41 | `U2CON0`, `U3CON0` mask and reset | 247, `0000-000` | 255, `00000000` (U1CON0 already 255) | MODE bit 3 of UART2/3 cannot be set. Fixed. |
| R7 | M | C | Q41; 16F13145 | `CRCCON0` mask | 255 | 251 (bit 2 absent) | Bit 2 writable. Check the other 16F parts with a CRC. **Not an error:** DS40002214G 13.12.1 and the register summary give SETUP[1:0] as bits 3:2, so bit 2 exists; mask 255 stays. |
| R8 | M | C | Q41 | `OSCSTAT` (0xB2) mask, reset | 253, `qqqqqq-q` | 255, `00000000` (all 8 bits read only) | Bit 1 reads as 0 and is missing. Fixed. |
| R9 | M | C | Q41 | `I2C1STAT0` (0x298) mask | 224 | 248 (7:3) | Bits 4:3 dropped although the fields exist. Fixed (248). |
| R10 | M | C | Q41 | `CCPTMRS0` (0x34C) mask, reset | 3, `------01` | 63, `--010101` | C2TSEL/C3TSEL dropped. Fixed. |
| R11 | M | C | Q41 | `DMASELECT` (0xE8) mask, reset | 3, `------00` | 7, `-----000` | Check the channel count of each part before widening. Fixed (mask 7; the Q41 data sheet lists SLCT[2:0]). |
| R12 | L | C | Q41 | `ADCON0` (0x3F3) mask | 221 | 213 (bit 3 absent) | Bit 3 writable. Fixed. |
| R13 | L | C | Q41 | `ADREF` (0x3F8) mask | 255 | 19 (bits 4, 1:0) | Reserved bits writable. Fixed. |
| R14 | L | C | Q41 | `PWM1ERS`-`PWM3ERS` mask | 255 | 15 | Fixed. |
| R15 | L | P | Q41 | `PWM1CLK`-`PWM3CLK`, `PWM1LDS`-`PWM3LDS` mask | 255 | 15 | Fixed. |
| R16 | L | C | Q41 | `NCO1CLK` mask | 255 | 239 (bit 4 absent) | Fixed. |
| R17 | L | C | Q41 | `I2C1CLK`, `I2C1BTOC` mask | 31, 15 | 15, 7 | Fixed. |
| R18 | M | P | Q41 | `DMAnSIRQ`, `DMAnAIRQ` mask, reset | 127, `-0000000` | 255, `00000000` | Check against the highest trigger number (93). Fixed. |
| R19 | M | C (Q41, DS40002214G 31.14.7) | Q41; 16F17146 | `CWG1AS1` mask, reset | 127, `-0000000` | 255, `00000000` | Q41 fixed (DS40002214G 31.14.7, DMAnSIRQ p.279). **Fixed 2026-11-05** for 16F17146 (DS40002343F 26.14.7 p.322: AS7E-AS0E all R/W, reset 0). |
| R20 | M | P | Q10 | `CCP1CAP`, `CCP2CAP` mask, reset | 3, `------00` | 15, `----0000` | CTS[3:2] cannot be set; `periph/ccp.rs` models only CTS = 0. **Fixed 2026-11-05** (CTS[3:0], DS40002043F 22.6.2 p.357). |
| R21 | M | P | Q10 | `T2CLKCON`, `T4CLKCON`, `T6CLKCON` mask, reset | 15, `----0000` | 31, `---00000` | Clock source 16-31 cannot be selected. **Fixed 2026-11-05** (CS[4:0], 21.9.5 p.343; T2/T4/T6). |
| R22 | M | P | Q10 | `CLKRCLK`, `SCANTRIG`, `MDCARH`, `MDCARL`, `MDSRC` mask, reset | 7, 15, 7, 7, 15 | 15, 31, 15, 15, 31 | One bit short each. **Fixed 2026-11-05** (CLKRCLK p.66 CLK[3:0], SCANTRIG p.192 TSEL[4:0], MDCARH/MDCARL CHS/CLS[3:0] p.437-438, MDSRC p.439 SRCS[4:0]). |
| R23 | L | P | Q10 | `CLC1SEL0`-`CLC8SEL3` (32 registers) mask | 255 | 63 | **Fixed 2026-11-05** (D1S-D4S[5:0], 26.8.3-26.8.6 p.418-421; reset `--xxxxxx`). |
| R24 | L | P | Q10 | `CWG1ISM` mask; `CWG1AS1` mask, reset | 31; 63, `--000000` | 15; 255, `00000000` | **Fixed 2026-11-05** (CWG1ISM ISM[3:0] p.400; CWG1AS1 AS7E-AS0E p.403). |
| R25 | M | P | Q41 | `DAC1CON`, `DAC2CON`, `OPA1CON0` mask | 60, 12, 168 | 189, 141, 171 (generic `DACxCON`/`OPAxCON0` page: instances differ) | Read both instances before changing. **Fixed 2026-11-05:** DS40002214G 41.5.1 p.763 (DAC1: EN, OE, PSS, NSS = 189), 41.5.2 p.764 (DAC2: EN, PSS, NSS = 141), 42.5.1 p.772 (OPA1CON0 = 171, bit 2 absent; `access` and reset also corrected). |
| R26 | L | P | Q41 | `OPA1OFFSET` / 16F `OPA1OFFSET` reset | `00000000` | `mmmmmmmm` (factory trim) | Reset is not zero; decide how to model. **Fixed 2026-11-05:** reset written as `mmmmmmmm` (DS40002214G 42.5.6 p.777). The loader sets only `1` bits, so the model still starts at 0 (no offset). 16F17126/46 done too (DS40002343F 34.5.6 p.546). |
| R27 | M | P | 16F152xx | `T1CLK` mask, reset; `T2CON` mask; `PCON1`, `BAUD1CON`, `FVRCON` reset | 15, `----0000`; 127; `------0-`, `01-00-00`, `01----00` | 31, `---00000`; 255; `------1-`, `00-00-00`, `00----00` | 16F15243; check the other 152xx parts. **Fixed 2026-11-05 for the six 152xx parts (one data sheet, DS40002195E):** `T1CLK` (20.11.3 p.174), `T2CON` (21.10.3 p.194), `PCON1` MEMV (10.12.3 p.75). **Not changed:** `BAUD1CON` (reset `01-00-00` kept: this data sheet lists RCIDL reset 0, the Q10 data sheet 29.6.6 lists 1, and an idle receiver reads 1) and `FVRCON` (`01----00` kept: its note 2 says FVRRDY is always 1); both look like data sheet errors. |
| R28 | L | P | 16F171x6 | `CLCSELECT` mask, reset; `OPA1CON0` mask | 3, `------00`; 168 | 15, `----0000`; 171 | 16F17146. **Fixed 2026-11-05** for 16F17126 and 16F17146 (DS40002343F 30.8.1 p.376 SLCT[3:0]; 34.5.1 p.541 OPA1CON0 = 171). |

Not errors, do not change: Q10 `PORTE`, `WPUE`, `INLVLE` (the data sheet covers the 47Q10, the 27Q10 has RE3 only); `INDFn`, `PLUSWn`,
`PREINCn`, `POSTINCn`, `POSTDECn` mask 0 (virtual registers); Q41 `TRISA` bit 3 (MCLR, read-only 1: the data sheet lists it as Reserved);
`OSCCON1/2` reset `f`/`q` letters (value comes from the configuration words).

## To fix: tooling and tests

| ID | Pri | Where | Finding |
|---|---|---|---|
| T1 | H | `Basic_Compiler/scripts/exec-test.py:46`, `scripts/dma_sim.py:290` | Work dirs from `tempfile.mkdtemp` (`basil-exec-*`, `basil-dma-*`) are never removed, even without `--keep`. This filled the 7.8 GB `/tmp` tmpfs (403 `basil-exec-*` dirs, plus 15 `basil-test-*` of 235 MB each from another test) and made every tool call fail. Remove the dir at exit unless `--keep`. **Fixed 2026-11-05:** `dma_sim.py` now removes its work dir at exit (`exec-test.py` already did); the Rust test scratch dirs moved to `target/tmp`. |
| T2 | M | `picsim/tests/sim/run.sh:17` | Calls `./target/release/basil`, which does not exist in picsim (the compiler is in Basic_Compiler). Make the path configurable or document the requirement. **Fixed 2026-11-05:** `run.sh` takes `BASIL=` (default `../Basic_Compiler/target/release/basil`); the blink step tolerates basil-sim exiting 1 at `--max-cycles`. |
| T3 | M | `Basic_Compiler/tests/exec/lib_pwm.bas` | MDB does not model `CCPxCON.OUT` / `PWMxCON.OUT`; the test now masks bit 5, so `OUT` itself is untested. Add a basil-sim-only test (` ' engine: sim`) that samples `OUT` across a period. **Fixed 2026-11-05:** `tests/exec/lib_pwm_out.bas` (`engine: sim`) samples `OUT` over 3000 reads; a duty of 100 instead of 401 makes it fail. |
| T4 | M | `Basic_Compiler/tests/exec_q41/lib_dma_abort.bas` | The test passes in basil-sim only because the DMA registers are plain storage there (no engine). It checks the library's register protocol, not the abort. The DMA engine remains MDB-only (and MDB is unreliable); no engine-level DMA test runs in basil-sim. |
| T5 | M | picsim `periph/ccp.rs` | Original report: a probe (Timer1 on Fosc/4 via `T1CLK = 1`, `CCPR1 = 5`, MODE 1000 "set on match", 1 ms wait) gave `CCP1CON` = 136 (OUT = 0) and `PIR6` = 0 in both basil-sim and MDB, so no compare match happened in either. Either the probe is wrong (clock select, PIR6 flag position, `T1CON` value) or compare mode has a bug. Rebuild the probe from the Q10 data sheet (DS40002043F ch. 22, 20) and settle it. **Fixed 2026-11-05:** **Resolved, no simulator bug.** The same probe built from DS40002043F (Table 20-2 p.306, Table 22-6 p.356, PIR6 p.205) gives `CCP1CON` = 168 (OUT = 1) and `PIR6` = 1 in basil-sim; `compare_set_on_match_with_a_small_count` in `crates/picsim/tests/ccp.rs` covers it. The earlier probe was wrong; its source was not kept. |
| T6 | L | `docs/FACTS-SIM.md` A61-A63 | The CCP assumptions are marked "to be measured against MDB"; with MDB unreliable they need a data sheet or hardware basis instead. **Fixed 2026-11-05:** A61-A63 now say the data sheet gives no timing and that MDB is not a reference; they stay modelling choices until measured on hardware. |
| T7 | L | `tools/audit/audit_sfr.py` | Not run in CI. Run it per part and fail on a confirmed list, or at least on any new difference, so masks and reset values stay tied to the data sheets. **Fixed 2026-11-05:** `tools/audit/run.sh` runs the audit for the Q10 and Q41 and fails on any difference not in `tools/audit/baseline/`; needs the PDFs, so it is a local check, not hosted CI. |

## Notes on the review itself

* Fixes already made were checked by `cargo test --release` (all pass), 148 Q10 and 11 Q41 exec programs under basil-sim.
* Concurrency / cache analysis of the mask change: masks apply at write time only (`mem.rs:119`, `pic18.rs:359-377`); peripheral flag
  setting (`Flag::set`) bypasses them, so widening a mask cannot change what the sim raises, only what software can write or clear.
  Interrupt dispatch reads PIR/PIE directly, so enabling previously masked bits (for example `PIE3.TX2IE`) now takes effect, as intended.
* The 16F descriptions are used by `devdesc` consumers only; basil-sim runs the Q10 and Q41.
* Not reviewed: the picsim peripheral models (`periph/*.rs`) beyond the paths above, and the 16F18xxx and remaining 16F parts.
