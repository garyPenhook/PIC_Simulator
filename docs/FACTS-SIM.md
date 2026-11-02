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
* Not modelled yet (milestone): interrupts and the Q41 hardware context (S3), reset values after MCLR/WDT (S3),
  TBLWT / flash and EEPROM writes (S5), every peripheral (S3-S5). SFRs without a model act as latches and are listed
  at the end of a run.

## Status (differential)

`scripts/exec-test.py --engine sim` (no MDB involved; the `' expect:` lines are the oracle):

* `tests/exec/*.bas`, 18F27Q10: 115 of 131 agree. The 16 that fail use interrupts, timers, UART, ADC, EEPROM/flash
  write or PWM (S3-S5).
* `tests/exec_q41/*.bas`, 18F16Q41: 10 pass, 5 fail (`lib_adc`, `lib_dma_abort`, `lib_eeprom`, `lib_flashwrite`,
  `lib_timer`: ADC, DMA, NVM and timer models), 4 `lib_dma_*` are skipped (the DMA replay needs MDB until S5).
* Not yet run: `--engine both` (cycle counts against MDB), PIC16 parts (S2).
