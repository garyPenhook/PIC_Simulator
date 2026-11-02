# Simulator facts (`crates/picsim`)

Each rule the simulator relies on, with its source. Rules marked `[A#]` are assumptions to be measured against MDB.

## PIC18 core (S1)

* Instruction encodings, flags affected and cycle counts: DS40002214G Table 45-2 (PDF pp.805-808). Cycles: 1; a
  taken skip adds the length in words of the skipped instruction (so 1-4); a taken branch, BRA, RCALL, CALL, GOTO,
  RETURN, RETFIE, RETLW, CALLW, MOVFF, LFSR, TBLRD, TBLWT take 2; MOVFFL takes 3.
* MOVFFL: source = word1<3:0>:word2<11:2>, destination = word2<1:0>:word3<11:0> (Table 45-2 field layout, read as
  most significant bits first) `[A1]`.
* LFSR literal: 14 bits (word1<3:0>:word2<9:0>) when FSRnH implements more than 4 bits (Q41), else 12 bits.
* When an instruction writes STATUS and also affects flags, the flags win (write happens first).
* A read-modify-write through `POSTINC/POSTDEC/PREINC/PLUSW` steps the FSR once (found by the `tests/exec` array
  programs, `addwf POSTINC0, f`).
* Access Bank: `f` below the length of `access_ram` maps to `access_ram.start + f`, above it to `access_sfr.start +
  (f - len)` (device file; matches the compiler's `Target`).
* The pack's `access` letters (`n r w c s -`) are **not decoded yet**: their meaning is not recorded in a data sheet
  here. Only the `implemented` mask is applied (unimplemented bits read 0 and ignore writes).
* Not modelled yet (milestone): reset values after MCLR/WDT, configuration words (S3 remainder), TBLWT / flash and
  EEPROM writes (S5), ports/UART/ADC (S4). SFRs without a model act as latches and are listed at the end of a run.

## Interrupts and timers (S3)

Code: `crates/picsim/src/pic18.rs` (`irq_check`, `retfie`), `periph.rs` (Timer0/1/2). Cites are in the module comments.

* Sources are every PIRn/PIEn/IPRn triple of the part. `IPEN = 0`: GIE (and PEIE for PIR1 and up on the Q10; PIR0 is
  ungated `[A5]`; the Q41 has no PEIE) enables all, vector 0008h. `IPEN = 1`: high needs GIEH, low needs GIEH and GIEL,
  vectors 0008h/0018h. DS40002043F 15.1-15.3 p.193-194; DS40002214G 11.5-11.8.
* Q10 entry clears GIE/GIEH (GIEL for low) and saves W/STATUS/BSR in the fast stack; `RETFIE` sets GIE/GIEH, or GIEL
  when leaving a low handler (p.194). Q41 (`SHADCON` present) leaves GIE alone, tracks the level itself, saves
  W, STATUS, BSR, FSR0-2, PROD, PCLATH/U per level and `RETFIE 1` restores them (FACTS-18F-Q41.md section 4).
* Entry timing, measured against MDB: the vector is reached 3 cycles after the cycle the flag was set, never less
  than 2 after the instruction `[A4]`. isr_single's first 27 entries and all `lib_timer*` cycle counts match MDB exactly.
* Timer0 8-bit compares TMR0L with the *live* TMR0H (a write right after the flag applies to the period that has just
  started; measured, MDB), period TMR0H+1 clocks; 16-bit buffers TMR0H. Timer1: FOSC/4 and FOSC, RD16 buffer.
  Timer2: free-running and software one-shot (MODE 00000 / 01000). Other clock sources, gates and hardware reset
  modes log a note and do not count/act (S4).
* Flag time agrees with MDB for software polling in all 12 probe variants (`[A1]`, `[A2]`).

### Known differences from MDB (the sim follows the data sheet)

* D-SIM-1: MDB loses part of the Timer0 prescaler count on a TMR0L *read* and on `movf T0CON0,f`. The data sheet
  clears the prescaler on writes only (DS40002043F 19.2.4 p.297), and `movf f,f` writes. Probe programs:
  `tests/hw/t0_*_sim.bas` (sim gives 402 / 401 / 500 / 401 for nowrite / readw / movf / tmr0l). `isr_single` differs by
  565 cycles because of it. Settle on silicon with the `_hw` flavours (results in EEPROM 0-1).
* D-SIM-2 (open): interrupt entry racing a `bcf INTCON,7` critical section differs from MDB by 1-8 cycles over a run
  (`bit_packed_isr` +4, `isr_dual` +2 total); MDB defers some requests the sim takes. Not yet understood.
* TMR0L read right after a TMR0H change in 8-bit mode differs by 1-2 counts.

## Configuration words (S3)

Decoded by `crates/picsim/src/config.rs` from the bytes the hex puts at the `[[config]]` addresses; a byte the image
lacks keeps the register's unprogrammed default (0xFF). `basil-sim --config` prints the fields and the SFRs below.

* `RSTOSC` sets the reset values of `OSCCON1` (NOSC, NDIV), `OSCCON2` (COSC, CDIV) and `OSCFRQ`. Q10: DS40002043F
  Table 5-1 p.53; Q41: DS40002214G Table 12-1 p.175. 111 = EXTOSC, 110 = HFINTOSC 4 MHz / 4, 101 = LFINTOSC,
  100 = SOSC, 010 = EXTOSC + 4xPLL (OSCFRQ 4 MHz). **At 000 the tables differ**: NOSC/COSC = 110 with OSCFRQ 64 MHz on the
  Q10, NOSC/COSC = 000 with OSCFRQ 64 MHz on the Q41. 011 and 001 are reserved and change nothing.
* `WDTCON0` resets to PS = `WDTCPS` (01011 when `WDTCPS` = 11111), SEN = 0: DS40002043F 10.8.1 p.102, DS40002214G
  15.7.1 p.240. `WDTCON1` resets to WINDOW = `WDTCWS` and CS = `WDTCCS` (000 when `WDTCCS` = 111): DS40002043F 10.8.2
  p.103, DS40002214G 15.7.2 p.241.
* `MVECEN` = 1 on the Q41 (vector table, DS40002214G CONFIG3 p.46) is not simulated; the first interrupt taken adds a
  note and goes to the legacy vector. The compiler refuses `MVECEN = ON` (`E-CONFIG`), so only a program that leaves
  CONFIG3 out (default 0xFF = ON) and still takes an interrupt sees it.
* `WDTE` <> 0 adds a note: the watchdog does not time out yet (`CLRWDT` is a no-op). Needs reset kinds (next item).
* `LVP`, `STVREN`, `XINST`, code protection and write protection have no effect in the simulator.
* Not yet applied: `FEXTOSC`/`CSWEN` (`OSCCON1` is writable regardless of `CSWEN`), the OSCSTAT ready bits.

## Status (differential)

`scripts/exec-test.py --engine sim` (no MDB involved; the `' expect:` lines are the oracle):

* After S3, excluding the ADC/EEPROM/flash/serial/DMA programs (S4/S5): 130 pass, 1 fails (`lib_timer_hw`, Timer1
  gate value bit GVAL, needs the pin model of S4).
* Not yet run: `--engine both` (cycle counts against MDB), PIC16 parts (S2).
