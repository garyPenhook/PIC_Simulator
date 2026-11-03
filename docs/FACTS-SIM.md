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
  EEPROM writes (S5). SFRs without a model act as latches and are listed at the end of a run.

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

## EUSART transmitter (S4, Q10 style)

Code: `periph.rs` (`Uart`), EUSART1 and EUSART2 (TXn/SPn/BAUDn names; the Q41 `Un*` UARTs are the next section).
DS40002043F ch.29 p.503-505.

* TXREG goes to the shift register at once when that is idle, else it is held until the Stop bit ends (29.1.1.2).
  TXIF = TXEN and nothing held (29.1.1.4); TRMT = shift register idle, read only (29.1.1.5). `RCIDL`/`ABDOVF` of
  BAUDnCON are read only and keep their value on writes.
* Character time in instruction cycles (Table 29-1): BRG16 = 1 and BRGH = 1: n+1 per bit; exactly one of them: 4(n+1);
  neither: 16(n+1); 10 bits, 11 with TX9. `[A6]` the held byte starts in the cycle the Stop bit ends.
  `[A7]` clearing TXEN abandons the shift register and a held byte; a TXREG write with TXEN = 0 is dropped.
  `[A8]` TXIF does not dip for a cycle after a write that goes straight to the shift register.
* Synchronous mode is not modelled (a note is logged; nothing is sent or received).
* `basil-sim` prints the transmitted bytes as `tx1: ...` / `tx2: ...`; `Periph::tx` holds (EUSART, start cycle, byte).

### Receiver (DS40002043F 29.1.2 p.507-510, RCxSTA 29.6.4 p.529)

* Two-character FIFO. RCIF = SPEN and CREN and an unread character (read only, follows the FIFO). FERR and RX9D describe
  the top character, so they change when RCREG is read. A third complete character with the FIFO full sets OERR and is
  lost; while OERR is set nothing more is received. Clearing CREN clears OERR; clearing SPEN also empties the FIFO.
* RX9 + ADDEN: characters with the ninth bit 0 are dropped without RCIF. RCIDL (BAUDnCON bit 6) is 0 while a character
  is in flight. A character whose Start bit falls while the receiver is off is lost (the hardware waits for the next
  falling edge).
* `[A9]` A character enters the FIFO when the Stop bit has been sampled, half a bit time into it: Start + data bits +
  half a bit time after the falling edge (9.5 bit times for 8 data bits). Not measured on hardware.
* `[A10]` Reading RCREG with an empty FIFO returns the last character read again (data sheet silent).
* `[A11]` Clearing CREN keeps the characters already in the FIFO (29.1.2.5 says they can still be read) and abandons the
  one in flight.
* RCREG is read by the instruction against the state after the previous instruction (peripherals sync once per
  instruction), so a character that completes inside the reading instruction shows up one instruction later.
* Auto-baud (ABDEN), break detection and the RX pin itself (RXxPPS, TRIS, ANSEL) are not modelled: the stimulus file
  drives the receiver input directly.

## UART of the PIC18-Q41 (S4, `periph/quart.rs`)

DS40002214G ch.34 p.519-566 (module `DOS-03076_uart_protocol_0_v1`), UART1..UART3 by register name (`U1CON0`, ...);
register layouts from 34.20 p.547-565. Asynchronous modes only (MODE 0 8-bit, 1 7-bit, 2 odd and 3 even parity in the
9th bit, 4 address). LIN, DMX, DALI (MODE > 4), auto-baud, break, wake-up, flow control and checksum are not modelled; a
note is logged when a program turns one on.

* Bit time in instruction cycles (Table 34-2 p.540, Fosc/4 per cycle): BRGS = 1: `UxBRG + 1`; BRGS = 0: `4(UxBRG + 1)`.
  `UxBRG` takes a write only while ON = 0 (34.20.8 note 2). A character is Start + 7/8/9 data bits + Stop (STP: 1, 1.5,
  2, 2 bit times, Table 34-1 p.536).
* Transmitter (34.2.1 p.519-520, 34.10 p.536): a `UxTXB` write goes to the shift register at once when that is free, else
  waits in `UxTXB` and moves at the beginning of the previous character's Stop bit; the next Start bit follows the last
  Stop bit. TXBE = buffer empty, TXBF = full, TXWRE (sticky, software clears) = write into a full buffer. TXIF = ON and
  TXEN and buffer empty. TXMTIF = shift register empty and idle, read only. Data written with TXEN = 0 waits in the
  shift register. TXBE = 1 written flushes the buffer (and an inactive shift register); a character being shifted
  finishes. ON = 0 discards everything. Address mode: a write to `UxP1L` sends the byte with the 9th bit set.
* Receiver (34.2.2 p.522-524, 34.8.1, 34.9 p.536): two-character FIFO. The character is stored in the middle of the first
  Stop bit (second with STP = 10); STPMD = 1 stores it at the end of the last Stop bit. RXIF = FIFO not empty and not
  suppressed by FERIF with FERIE or PERIF with PERIE. FERIF/PERIF describe the top character (PERIF = parity error in
  MODE 2/3, the 9th bit in MODE 4). A character completing with the FIFO full is lost and sets RXFOIF, which stays until
  software clears it. RXBE = 1 written flushes the FIFO. UxEIF = any `UxERRIR` flag with its `UxERRIE` bit.
* The stimulus file feeds it like the EUSART: `[[uart]]` with `port` = the UART number; `nine = true` sends the 9th bit
  (bit 8 of each `data` value) for the parity and address modes. Transmitted bytes print as `tx1:` ... as before.
* `[A18]` UxTXIF follows a write at once. Figure 34-3 p.521 marks "1 TCY" on it, which may be a one-cycle dip (a dip
  would matter only to code polling TXIF straight after a write; `lib/serial` polls TXBF).
* `[A19]` Clearing TXEN lets the character being shifted finish and starts no further one; a held character is sent when
  TXEN is set again. A `UxTXB` write with ON = 0 is stored and ignored.
* `[A20]` ON = 0 resets TXMTIF to 1, the error flags and TXWRE to 0 and empties both buffers.
* `[A21]` A character whose Start bit falls while the receiver is off (ON, RXEN, unsupported MODE) is lost. With RXFOIF
  set: RUNOVF = 0 stops the receiver (the character is lost), RUNOVF = 1 receives it and discards it. RXIDL is low from
  the Start bit until the character is stored.
* `[A22]` An empty FIFO reads the last character again. Address mode stores every character (address matching is not
  modelled; errata DS80000901H 1.5.2 says not to use the mode).
* `[A23]` A `UxTXB` write into a full buffer is dropped. In MODE 1 the top bit is not sent.
* Hardware-owned bits: TXMTIF, FERIF and PERIF of `UxERRIR` ignore writes (they follow the state); the other flags take
  the written value (R/W/S), so software clears RXFOIF with a write of 0. A `bsf`/`bcf` on `UxFIFO` writes TXBE and RXBE
  back as read, so on an empty buffer it flushes (the data sheet's note on `UxFIFO` says the same).
* `lib_serial` (Q41) passes on `--engine sim` as before; `tests/sim/run.sh` adds the library receiving three bytes and a
  framing error (`serial_rx_q41`) and sending two bytes with `SerialFlush` (`serial_tx_q41`, sim only: MDB never
  drains the Q41 transmit buffer).

## ADC (S4, `Adc` in `periph.rs`; Q10 only)

Basic mode of the ADCC, PIC18F27/47Q10 (DS40002043F ch.33). Other parts get a note ("the ADC of this part is not modelled")
until their timing is read from their own data sheet.

* ADGO = 1 with ADON = 1 starts a conversion (33.2.1 p.551). It lasts ADPRE + ADACQ ADC clocks (0 = not included,
  33.7.9 p.575, 33.7.10 p.576) plus **11.5 TAD** (33.1.4 p.549; the lib comments say 12). TAD = 2(ADCLK+1) Fosc periods
  (Table 33-2 p.549), so a conversion takes ceil((ADCLK+1)(2(ADPRE+ADACQ)+23)/4) instruction cycles.
* On completion (33.2.2 p.551): ADRES = new code (10 bits; ADFM = 1 right justified, 0 left justified, zero filled, Figure
  33-3 p.551), ADGO cleared unless ADCONT = 1 (then the next conversion starts at once), ADIF set. Clearing ADGO or
  ADON terminates the conversion and ADRES keeps its value (33.2.3 p.552).
* Input: the `[[adc]]` value of the channel in ADPCH at the end of acquisition (latest entry whose `at` has passed; 0 when
  none), clamped to the resolution.
* `[A12]` the conversion starts in the cycle of the instruction that sets ADGO; `[A13]` ADGO written 1 with ADON = 0 reads 0
  and starts nothing; `[A14]` an ADCONT retrigger starts in the cycle the previous conversion ended.
* Not modelled (note or ignored): FRC clock (ADCS = 1; ADGO never clears, with a note), computation (ADPREV, ADACC,
  filters, thresholds, ADTIF), triggers (ADACT), double sampling, ADCAP/guard/precharge polarity, the pin and ANSEL state.
* FVRCON: FVRRDY (read only) follows FVREN. `[A15]` at once; the data sheet gives only FVR04, a typical 25 us start-up
  (p.707), and the periph models do not know Fosc. A note is added when it is enabled.
* Tests: `crates/picsim/tests/adc.rs`, `lib_adc` and `lib_adin` on `--engine sim`.

### Q41 (PIC18F0x/1xQ41, DS40002214G ch.40)

Same ADCON0 bits (ON 7, CONT 6, CS 4, FM 2, GO 0; 40.7.1 p.735), ADCLK n -> TAD = 2(n+1) Fosc (40.7.6 p.740), ADIF set and
GO cleared on completion (40.2.2 p.720). Differences from the Q10:
* ADPRE and ADACQ are 13 bits (ADPREH/L, ADACQH/L) counted in **Fosc clocks, not ADC clocks** (40.7.9 p.744, 40.7.10 p.745);
  ADACQ = 0 with ADPRE != 0 means 8192 clocks. `[A18]` Fixed in 0.1.123: the library used to write ADACQ = 8 as "8 ADC clocks"
  (8 Fosc, 125 ns at 64 MHz, not 8 us); it now writes `ceil(FOSC * AcqUs / 1e6)` Fosc clocks on the Q41 and the PIC16F171xx
  (512 at 64 MHz for 8 us). The Q10 still counts ADC clocks.
* Conversion = 14 TAD + 2 TCY after acquisition (AD21 p.903, Fosc clock source); precharge + acquisition = (ADPRE + ADACQ)/4 cycles.
  `[A19]` AD21 is a "typ" design-guidance value; the model takes the 2 TCY as extra to the 14 TAD and starts counting at GO.
* 12-bit result (FM = 1 right justified, FM = 0 shifted up four places, Figure 40-3 p.720); ADPCH is 8 bits (40.7.8 p.742).
* `[A20]` GO written 1 by the instruction that sets ON (ON was 0) is ignored (p.720 "will not be set in the same instruction").
* Same not-modelled list as the Q10 (ADCRC, computation, double sampling, ADACT, ADCP/CPRDY). Errata DS80000901H: 1.1.1 (ADC
  dead on LFINTOSC/SOSC with BOR and FVR off) and 1.1.2 (double sample) are not modelled.
* Tests: `q41_*` in `crates/picsim/tests/adc.rs`, `tests/exec_q41/lib_adc.bas` on the sim, `tests/sim/adc_q41.bas` (stimulus).

## Ports and pins (S4)

* `PORTx` read gives the pin levels, a write goes to `LATx` (DS40002043F ch.17: the pin is read, the latch written). An
  output pin (TRISx = 0) shows its latch; an input pin shows what the stimulus drives, 0 when its ANSELx bit is set
  (digital input buffer off). A read-modify-write on PORTx therefore reads the pins and writes the latch, as on the part.
  `[A16]` An input nobody drives reads 0; weak pull-ups (WPUx), open-drain (ODCONx) and the input level thresholds are
  not modelled; a pin driven by a peripheral output (RxyPPS) is not either.
* Timer1 GVAL (T1GCON bit 2): the gate source after GPOL, for the pin source (T1GATE = 0, pin chosen by T1GPPS, reset
  RB5 on the Q10). `[A17]` MDB reads GVAL = 0 while GE = 0 and the polarity-adjusted pin level while GE = 1, whatever
  GTM, GSPM, GGO and ON are (probed on all combinations on a low pin); the data sheet says GVAL is unaffected by GE
  (DS40002043F 20.14.2 p.318). The model follows MDB. Other gate sources read as low, with a note. The gate does not
  yet stop the counter.

## Stimulus file (`crates/picsim/src/stimulus.rs`, `basil-sim --stim FILE`)

TOML; `[[uart]]` entries (`port`, `at`, `bytes` or `data`, `bit`, `gap`, `nine`, `bad_stop`), `[[adc]]` entries (`channel`, `value`, `at`), `[[pin]]` entries (`port` letter, `bit`, `level`, `at`) and `[[port]]` entries (`port`, `value`, `mask`, `at`); documented at the
top of `stimulus.rs`. Characters of one entry go out back to back; entries follow each other on the line. `at` is an
instruction cycle, `bit` the sender's bit time in instruction cycles (default: the receiver's own rate at the time the
character starts). `at`, `bit` and `gap` are capped at 2^32 cycles (larger values are a parse error). The stimulus is kept
and applied again after `Machine::reset()`. Tests: `cargo test --release -p picsim`, `tests/sim/run.sh` (a Basil program reading from it).

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

* `lib_timer_hw` passes since the pin model (GVAL), with MDB's cycle count.
* Not yet run: `--engine both` (cycle counts against MDB), PIC16 parts (S2).

## Data EEPROM of the Q10 (S5, `periph/nvm.rs`, DS40002043F ch.12)

* Read: NVMADRU:H:L then RD; NVMDATL holds the byte on the next instruction; works with NVMEN clear (12.3.1 p.157, 12.5.1 p.162).
* Write: NVMCON2 = 55h, AAh, then WR = 1, only with NVMEN set; erase-before-write; the CPU keeps running; hardware clears
  WR and sets NVMIF (PIR7.5) when done (12.1.2 p.147, 12.3.2 p.158). Clearing NVMEN afterwards does not stop it.
* A write to an invalid address clears WR and sets NVMERR (12.3.4 p.159). NVMCON2 reads zero (12.5.3 p.164).
* Time: MEM24 10 ms typ, 11 ms max (39.3.5 p.697). `[A30]` typical, in instruction cycles (`Nvm::write_cycles`, default
  10 000 = Fosc 4 MHz; the models do not know Fosc). `[A31]` unlock tracking ignores instructions between the writes.
  `[A32]` not modelled: PFM read/write, sector operations, WRTD write protect, EEPROM content across power-on reset.
* Errata DS80000832G 1.6.1 (NVMERR stays set across Resets) is not modelled; the search of the errata found no other NVM item.

## Data EEPROM of the Q41 (S5, `periph/nvmq.rs`, DS40002214G ch.10)

* NVMCON1.NVMCMD picks the operation, NVMCON0.GO starts it (Table 10-1 p.87-88). Read 000 and read with post increment 001 need
  no unlock and set no NVMIF; the byte is in NVMDATL on the next instruction (10.4.1 p.101). Write 011 / write with post
  increment 100 need NVMLOCK = 55h, AAh before GO, erase first, let the CPU run, and on completion clear GO and set NVMIF
  (PIR0.4) (10.2 p.88, 10.4.2 p.102-103).
* While GO is set, writes to the NVM registers are blocked and GO cannot be cleared by software (10.1 p.88).
* WRERR (NVMCON1.7): a write while locked, to an unimplemented area, or a page operation on the DFM sets it and the GO attempt
  fails (10.5.2 p.106, 10.4.5 p.104). Software can clear it but not set it. NVMLOCK reads zero (10.5.3 p.107).
* Time: MEM24 gives 11 ms maximum and no typical value (48.3.5 p.895). `[A33]` `NvmQ::write_cycles` = 10 000 cycles (Fosc 4 MHz).
  `[A34]` post increment adds 1 on the DFM, at completion for writes (the data sheet does not say when). `[A35]` unlock tracking
  ignores instructions between the writes. `[A36]` not modelled: PFM and page operations, WRTn protection, WRERR after a Reset
  during a write, blocking of NVMADR/NVMDAT writes while GO is set.
* Errata DS80000901H: the search found no NVM item. `exec_q41/lib_eeprom` passes (30439 cycles, not yet compared with MDB).

## Program flash self-write (S5, `periph/flash.rs`, `nvm.rs`, `nvmq.rs`, `Pic18::tblwt`)

Q10, DS40002043F 12.1 p.142-156; Q41, DS40002214G 10.3 p.88-98.

* Sector (Q10) or page (Q41) = 128 words = 256 bytes, selected by NVMADR[21:8]; NVMADR holds the byte address (Table 12-2 p.142,
  10.3.1 p.89). The holding registers / buffer RAM are the 256-byte GPR bank after the last GPR bank (12.1 p.142; Table 10-2 p.92):
  bank 13 on the 18F27Q10, bank 9/13/21 on the Fx4/Fx5/Fx6Q41. The device files mark it `system`, which is where the model reads it.
* `TBLWT` writes TABLAT to the holding register / buffer byte TBLPTR[7:0] selects, not to flash; the pointer moves as for `TBLRD`
  (12.1.4 p.151, 10.3.10.5 p.99). The sector/page read fills the holding bytes; the sector/page write programs from them.
* Q10 control bits and unlock pairs (Table 12-4 p.147): WR 55h/AAh, SECWR DDh/22h, SECER CCh/33h, SECRD BBh/44h. They are gated by
  NVMEN, RD is not (12.1.4.5 p.156); an unlock for one operation does not enable another. Q41: NVMCMD 000/001 read, 010 page read,
  011/100 word write, 101 page write, 110 page erase, unlock NVMLOCK 55h/AAh (Table 10-1 p.87-88).
* The CPU stalls until a PFM erase or write ends; the control bit/GO is clear when it resumes and NVMIF is set. Reads and table writes
  set no NVMIF. An invalid address sets NVMERR (Q10) or WRERR (Q41) and the operation does not happen.
* Q10 times, typical (39.3.5 p.697): word write 50 us, sector write 6 ms, sector erase 10 ms. Q41 gives maxima only (48.3.6 p.896):
  word write 75 us, page write 10 ms, page erase 11 ms.
* `[A37]` the Q10 sector read takes no time. `[A38]` the Q41 uses its maxima. `[A39]` a Q41 page read takes no time and post increment adds 2
  to NVMADR on the PFM. `[A40]` flash programming only clears bits: a write ANDs into what is there, with a note when it would need 0 -> 1.
  All times scale with `write_cycles` (10 000 = 10 ms at Fosc 4 MHz, A30/A33).
* Not modelled: write protection (WRTn), User ID and configuration space (such operations are ignored with a note), NVMERR/WRERR after
  a Reset during a write. Errata DS80000832G 1.9.1 (the first instruction after a self-write instruction may not execute; the library adds
  two NOPs, p.7) is not modelled either, so code without the NOPs would run on the simulator and fail on silicon.
* Results: `exec/lib_flashwrite` (18F27Q10, 28046 cycles) and `exec_q41/lib_flashwrite` (18F16Q41, 34286 cycles) pass on the sim; neither
  is compared with MDB. `tests/exec/lib_flashwrite.bas` in Basic_Compiler was changed: with flash reads modelled R3 is 1 as on hardware
  and the sector path is exercised by a second update (R5, R6).

### NVM against MDB (`exec-test.py --engine both`)

* D-SIM-3: MDB does not model self-write durations. `lib_eeprom` takes 426 cycles (Q10) / 451 (Q41) in MDB against 30414 / 30439 in the
  sim; `exec_q41/lib_flashwrite` 2061 against 34286. The `' expect:` values of those three agree on both engines; only the cycle count
  differs, by the write waits (MDB completes a write at once). `--engine both` therefore reports FAIL for NVM programs on cycles alone.
* D-SIM-4: `exec/lib_flashwrite` (Q10) fails in MDB itself (`FW_R3` 0 not 1, `FW_TA` 86 not 18, `FW_ER` 16 not 0): MDB does not model the
  program flash reads and the self-write the way the data sheet describes, so the sim (which passes) cannot be checked against it there.
* Not compared: write times against hardware (the `[A30]`-`[A38]` times stay data sheet maxima), errata 1.9.1.


## SPI of the PIC18-Q41 (S5, `periph/spi.rs`, DS40002214G ch.35)

Host mode only; the outside world is a byte-level client from `[[spi]]` in the stimulus file (`data`, `fill`, `port`); the bytes the host
sends are printed by `basil-sim` as `spiN: ...`. Chip select (SS_out, SSET), SS_in, PPS routing and client mode are not modelled.

* Sourced: transfer conditions by TXR/RXR and BMODE (Table 35-1 p.575), counter and TWIDTH (35.7.6-7 p.594-595), SCK = clock / (2 (BAUD + 1))
  (Eq. 35-1 p.579), start delay at least half a baud period unless FST (35.3.6.3 p.581), registers locked while EN = 1 (35.2.1 p.571), flags
  SPIxRXIF / SPIxTXIF / SPIxIF (35.6 p.586), STATUS and INTF bits (35.7.8, 35.7.12 p.596, 600).
* `[A40]` both FIFOs are two bytes deep (Figure 35-3 shows occupancy up to 2; the depth is not stated).
* `[A41]` a byte leaves the TxFIFO and enters the RxFIFO when its transfer ends.
* `[A42]` only CLKSEL = 0 (FOSC) is modelled; start delay `ceil((BAUD + 3) / 4)` cycles (1 with FST), no gap between back-to-back bytes,
  a byte of w bits takes `ceil(w (BAUD + 1) / 2)` cycles.
* `[A43]` bytes narrower than 8 bits use the low w bits; SDIP inverts the client's reply; LSBF and CKP/CKE/SMP do not change the data.
* `[A44]` a TXB write with EN = 0 is dropped.
* Tests: `crates/picsim/tests/spi.rs`; `tests/sim/spi_q41.bas` runs the library `Spi` module against a client.

## MSSP of the PIC18-Q10 (S5, `periph/mssp.rs`, DS40002043F ch.28)

SPI host and I2C host. Slave modes, the firmware controlled I2C host (SSPM = 1011) and multi-master bus collisions are not modelled (a
note is printed; BCLxIF is never set). PPS, TRIS and the pins are not modelled: the module runs once SSPEN is set. The outside world comes
from the stimulus file: `[[spi]]` (same as the Q41 SPI: `port` = MSSP number, `data`, `fill`) answers each exchanged byte, and `[[i2c]]`
(`port`, `address` 7-bit, `data`, `fill` default FFh; several entries make several clients) are clients on the bus. `basil-sim` prints
`spiN: ...` (bytes sent) and `i2cN: S 50W 10 Sr 50R CA FE! P`: 7-bit address with R/W, `!` after a byte nobody acknowledged (address or
write) or after a byte the host answered with NACK; `S`, `Sr`, `P` are the conditions. The bus has pull-ups: an address nobody owns is
NACKed and a read from nobody gives FFh.

* Sourced: SPI starts on the SSPxBUF write, a write while shifting is dropped with WCOL, BF and SSPxIF at the end, a read of SSPxBUF clears
  BF (28.2 p.444-445); clocks FOSC/4, /16, /64, FOSC/(4 (SSPxADD + 1)) (SSPxCON1 p.494, Eq. 28-1 p.489); I2C no queuing (28.6 p.474-475),
  Start / Repeated start / Stop / Acknowledge sequences (28.6.4-28.6.9 p.476-482), transmit BF / ACKSTAT / SSPxIF (28.6.6 p.477-478), receive
  RCEN / BF / SSPOV (28.6.7 p.479-480), S and P cleared by a disabled module (28.6 p.474), STAT.R/W = transmit in progress (p.492).
* `[A45]` the SPI transfer starts in the cycle of the write and takes 8 bit times (1, 4, 16 or SSPxADD + 1 cycles a bit); TMR2 / 2 as
  clock runs as FOSC/4 with a note.
* `[A46]` I2C TBRG = (SSPxADD + 1) / 2 cycles (BRG at FOSC/2, Fig. 28-40 p.489), sequences of k TBRG rounded up: Start 2 (S at 1), Repeated
  start 3 (S at 2), Stop 3 (P at 2), byte sent 18 (BF clears at 16, ACKSTAT at 17), byte received 16, Acknowledge 2. The data sheet gives the
  steps, not the counts.
* `[A47]` several SSPxCON2 enable bits written at once: the lowest of SEN, RSEN, PEN, RCEN, ACKEN starts, the others are dropped.
* `[A48]` the first overflow (BF set, SSPOV clear, BOEN = 0) still updates SSPxBUF and sets SSPOV; the data sheet says SSPxBUF is updated
  only while SSPOV is clear.
* `[A49]` a client acknowledges its address and every byte written to it; clearing SSPEN or changing SSPM aborts the running operation and
  clears BF, S, P, R/W (WCOL and SSPOV stay). WCOL and SSPOV are set by hardware only; software writes can clear them. A rejected SSPxBUF
  write (WCOL) leaves SSPxBUF unchanged. Two `[[i2c]]` clients with the same address on one port are a stimulus error.
* `[A50]` an SSPxBUF write with SSPEN = 0 only fills the buffer.
* Tests: `crates/picsim/tests/mssp.rs`; `tests/sim/spi_q10` and `tests/sim/i2c_q10` run the library `Spi` and `I2c` modules (compiled with
  `basil`) against clients.
