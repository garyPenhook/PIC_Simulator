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
  (DS40002043F 20.14.2 p.318). The model follows MDB. Other gate sources read as low, with a note.
* Timer1/3/5 gate `[A103]` (DS40002043F 20.7 pp.309-313, Table 20-3, source table 20-4 p.310; Q41: DS40002214G 25.7, Table 25-5
  p.378): with GE the timer counts only while the gate source, after GPOL, is active; a closed gate holds the count and
  (assumed, not in the data sheet) the prescaler. GTM: the gate is a flip-flop that toggles on the rising edge. GSPM:
  counting starts at the first rising edge after GGO is set and stops at the next falling edge, which clears GGO (also
  cleared with GSPM). TMRxGIF is set on a falling edge of the gate, also with GE = 0; in single-pulse mode only for the
  counted pulse. GVAL itself reads the plain source level (see above). Counting in Sleep uses the same plan.
  * Sources modelled: the T*GPPS pin (GSS = 0) and the CCP, PWM (Q10), comparator and CLC outputs. They are followed through
    the span since the last look: stimulus steps for a pin, the log of PPS output source changes (`sig_log`, kept back to
    the oldest timer look) for the others. A change on cycle t is seen by clock ticks on cycle t and later (a source edge
    in tick q of cycle c counts from cycle c+1 unless q = 0). A timer gated by an output source is advanced after the
    CCP/PWM/comparator edges of the span have been made, so that it sees them in the span they fall in.
  * A change that software makes to the pin (LATx, TRISx, ANSELx, RxyPPS, or the PORTx write) is synced up to the write
    and seen from the next cycle (`Special::Sync`, only while a Timer1-type timer is ON or has GE set). A write to
    TxGATE or TxGPPS ends the span first and is not an edge of the gate; so is a write to TxGCON (a GPOL change).
  * Not modelled (the timer counts regardless, with a note): the timer overflow and post-scaled outputs (the data sheet gives
    no pulse width), ZCD, NCO, SMT and the Q41 PWM as gate sources. A gate pin driven by a peripheral through RxyPPS is read
    at the level of the end of the span, not time-resolved. With the timer OFF and GE = 0 a pin change is not synced, so
    TMRxGIF sees it at the next look, one cycle after the span starts.
  Tests: `timer1_*gate*` in `crates/picsim/tests/pins.rs`.

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
* `[A51]` Q41 I2C host (`periph/i2cq.rs`, DS40002214G ch.36): a TXB write with TXBE = 1 clears TXBE at once; the first write starts the Start
  after the bus free time; the address moves to the shift register one SCL period after the Start (SCIF set, MMA = 1). SCL period =
  5 (4 with FME) x (BAUD + 1) cycles; I2CxCLK other than FOSC/4 logs a note and runs as FOSC/4.
* `[A52]` at the 8th edge of a transmitted byte with TXBE = 1 and I2CxCNT != 0 the host sets MDR and stretches until TXB is written.
* `[A53]` at the 9th edge after an ACK a pending TXB byte moves into the shift register and the count decrements; with nothing pending and
  count 0, CNTIF is set and the Stop follows (RSEN = 0) or MDR is set (RSEN = 1).
* `[A54]` a NACK from the client sets NACKIF and ACKSTAT; RSEN = 0 stops by itself, RSEN = 1 sets MDR and waits.
* `[A55]` a TXB write or S = 1 at the MDR pause issues the Restart; RSCIF one SCL period later, the address moves to the shift register two periods later.
* `[A56]` host receive: RXB/RXBF/count at the 8th edge, MDR stall at the 7th edge of the next byte while RXBF = 1; the 9th clock sends ACKDT
  (count != 0) or ACKCNT (count 0); a NACK before the count ends is a Stop; the host's own NACK sets NACKIF.
* `[A57]` P = 1 while MMA = 1 stops within one SCL period and clears MDR.
* `[A58]` EN = 0 resets the state machine (count kept); on EN = 1 SCIF and PCIF are set 3 cycles later (errata); BFRE = 8 prescaled clocks after
  the enable or a Stop.
* `[A59]` TXB written with TXBE = 0 sets TXWE and NACKIF and is dropped; RXB read with RXBF = 0 sets RXRE and NACKIF and returns the old byte.
* `[A60]` TXB written with EN = 0 or MODE != 100 only fills TXB. Client modes, 10-bit addressing, bus time-out, collisions and clock
  stretching by a client are not modelled.
* Tests: `crates/picsim/tests/mssp.rs`; `tests/sim/spi_q10` and `tests/sim/i2c_q10` run the library `Spi` and `I2c` modules (compiled with
  `basil`) against clients.

## Q10 CCP1/CCP2 and the pin-output layer (`periph/ccp.rs`, DS40002043F ch.22 p.345-359, 18.2 p.286-287)

* Pin-output layer: `RxyPPS` selects a source (Table 18-2; CCP1 = 0x05, CCP2 = 0x06). A pin with a modelled source and TRIS clear
  shows the source level on PORTx and every change goes to `Periph::pin_log` as (Fosc tick = 4 * cycle + quarter, port, pin, level);
  `basil-sim` prints it as `pins:`. Changes made to PPS/TRIS while a source is already high or low are not logged. Other sources
  (EUSART, MSSP, CWG, ...) are not driven by this layer yet and the pin keeps showing its latch.
* Modelled: capture (CTS = 0000, the CCPxPPS pin, MODE 3-7, 1:4 and 1:16 prescaler), compare (MODE 1, 2, 8, 9), standard PWM
  (pin set at the Timer2 period match, cleared after CCPR * TOSC * prescale; FMT alignment; duty 0 never sets). CCPTMRS picks the timer: Timer1/3/5 for
  capture and compare, Timer2/4/6 (FOSC/4) for PWM (reserved code 00 and timers the part lacks give a note).
* `[A61]` Compare match on the Timer1 clock that makes TMR1 = CCPRx; with clearing (MODE 1, 11) TMR1 goes to 0 on the next clock,
  so the period is CCPRx + 1 clocks. Basis: DS40002043F 22.3 (p.348) says only that a match sets the flag and drives the pin and that
  MODE 0001/1011 reset TMRx; it gives no clock-exact timing. This is a modelling choice, to be checked on hardware (MDB is not a reference).
* `[A62]` In PWM mode CCPxIF is set when the pin clears. Basis: none beyond Figure 22-4's "set CCPIF" arrow; PIR6 (p.205) calls the flag
  unused in PWM mode. Modelling choice, to be checked on hardware.
* `[A63]` Pulse output (MODE 10, 11) sets the flag (and clears TMR1 for 11) but does not drive the pin: the data sheet (Table 22-6, 22.3)
  gives no pulse width.
* Timers 3-6 are the same models as Timer1 (3, 5: 16-bit, T3GATE/T3GPPS gate table DS40002043F p.311) and Timer2 (4, 6); their
  flags are TMRxIF. Hook codes 226-235 and 244-253 are the extra instances.
* Not modelled: capture sources other than the pin, the ADC auto-conversion trigger from a compare, PWM3/PWM4. A CCPTMRS write
  while a PWM edge is scheduled does not cancel that edge.
* Tests: `crates/picsim/tests/ccp.rs`.

## Q10 CWG1 (`periph/cwg.rs`, DS40002043F ch.25 p.377-405)

* Data input CWG1ISM 1-4 = CCP1, CCP2, PWM3, PWM4 (Table 25-1); outputs CWG1A..D are PPS codes 0x01-0x04 (Table 18-2). Other inputs (pin, CMP, DSM, CLC)
  are not modelled and add a note. Hook codes 41-47 are the CWG1 registers.
* Modes (CWG1CON0 MODE): 0 asynchronous and 1 synchronous steering (STRy = 1 follows the input with POLy, STRy = 0 shows OVRy; only STR is
  synchronised, at the next rising edge), 2/3 forward/reverse full bridge, 4 half bridge, 5 push-pull. 6/7 reserved: outputs inactive.
* Dead band (half bridge; full bridge after a direction change): DBR delays the turn-on of A/C, DBF that of B/D; an input that turns round
  first shows no pulse. Count buffers: loaded on write with EN = 0; with EN = 1 at the rising edge after the first falling edge following LD = 1.
* Auto-shutdown: SHUTDOWN set by software or by the AS0E pin source (low); CWG1IF set on the change; LSAC/LSBD override levels (tri-state lets the
  pin show its latch); steering modes override only STRy = 1 pins; software clear refused while a source is active; outputs resume at the next
  rising edge; REN = 1 clears SHUTDOWN by hardware.
* `[A73]` Dead band is exactly DBx Fosc periods (the data sheet says DBx to DBx + 1); CS = 1 (HFINTOSC) is counted as Fosc with a note.
* `[A74]` After EN goes 1 the modulated modes keep the inactive level until the first rising edge; steering follows the input at once.
* `[A75]` EN = 0 drives all four outputs low.
* `[A76]` The AS0E pin is sampled on CWG register writes and input edges only; AS1E-AS7E (Timer2/4/6, CMP, CLC) are not modelled (note).
* `[A77]` POLy = 1 inverts (register description p.398); 25.6.2 says the opposite and is not followed.
* `[A78]` A full-bridge direction change (MODE[0] written while EN = 1) takes effect at the next rising edge; the new modulated output waits DBR
  (forward to reverse) or DBF (reverse to forward).
* A write to a CWG register while a dead-band turn-on is pending drops it and applies the steady-state levels at once.
* Tests: `crates/picsim/tests/cwg.rs`.

## Q10 comparators C1/C2 (`periph/cmp.rs`, DS40002043F ch.34 p.589-600)

* Stimulus `[[analog]] pin = "RA0" volts = 1.5 at = 0` (0..6 V, 0 V until set, later entries replace earlier ones) gives pin voltages. FVR_Buffer2
  follows FVRCON (FVREN and CDAFVR = 1.024 / 2.048 / 4.096 V). AVSS is 0 V.
* Output = (CxVP > CxVN) XOR POL; a disabled comparator makes a 0 before POL, so changing POL or EN while disabled gives an edge (34.5 note).
  OUT (CMxCON0 bit 6) and MCxOUT (CMOUT) follow it and are read only; CxIF (PIR2 bit 0 / 1, p.201) is set on a rising edge with INTP and a falling edge
  with INTN. PPS codes 0x0D (CMP1) and 0x0E (CMP2); CWG1 data inputs ISM 5 / 6 and shutdown sources AS4E / AS5E (output low).
* Pins (28-pin Table 3 p.5): CxIN0- RA0, CxIN1- RA1, CxIN2- RB3, CxIN3- RB1; CxIN0+ RA2; C1IN1+ RA3; C2IN1+ RB0.
* `[A79]` Voltages come from the stimulus only; VDD, ANSELx and TRISx are not looked at. `[A80]` No offset, hysteresis (HYS is stored), response time or
  Timer1 synchronisation (SYNC adds a note). `[A81]` The DAC input (PCH = 101) and not-connected selections read 0 V (DAC adds a note). `[A82]` 28-pin pins only.

## CLC1-CLC8 (DS40002043F ch.26 p.406-426; `periph/clc.rs`, `tests/clc.rs`)

* Registers p.414-426: CLCxCON EN 7, OUT 5 (read only), INTP 4, INTN 3, MODE 2:0; CLCxPOL POL 7, G4POL..G1POL 3:0; CLCxSEL0-3 D1S-D4S 5:0;
  CLCxGLS0-3 bit 2(k-1)+1 = GyDkT, bit 2(k-1) = GyDkN; CLCDATA MLCxOUT bit x-1. Output PPS codes 0x18-0x1F (Table 18-2 p.287); a disabled cell outputs 0.
* Gate = (OR of the enabled terms dkT/dkN) XOR GyPOL (Figure 26-2 p.408, Table 26-1 p.410: GLS 0x00 gives GyPOL). The prose of 26.1.2 says "ANDed"; the figure
  ANDs each term with its enable bit and ORs the terms, and the table agrees with that.
* Functions (Figure 26-3 p.411): 000 (g1&g2)|(g3&g4); 001 (g1|g2)^(g3|g4); 010 g1&g2&g3&g4; 011 S-R S=g1|g2 R=g3|g4; 100 D-FF D=g2 CLK=g1 S=g4 R=g3;
  101 D-FF D=g4|g2 CLK=g1 R=g3; 110 J-K J=g2 K=g4 CLK=g1 R=g3; 111 latch D=g2 LE=g3 S=g4 R=g1. CLCxIF on the rising (INTP) or falling (INTN) edge of the output (26.2).
* `[A83]` Evaluated after every instruction and on every change of a modelled input (pin stimulus times, PPS source changes); no propagation delay (39.4.20 not used).
  `[A84]` R wins over S when both are 1 (not stated). `[A85]` Disabling clears the flip-flop; enabling takes the present clock level as the previous one; enabling or disabling can itself be an output edge that sets CLCxIF per INTP/INTN (not described). A loop of cells that does not settle logs its edges on every pass.
  `[A86]` Inputs modelled: CLCIN0-7 pins, CCP1/2, PWM3/4, CMP1/2, CWG1A/B, CLC outputs. Others read low and add a note. `[A87]` Loops are iterated at most 8 times.
* Not modelled: ADC auto-trigger, Timer2/4/6 reset from a comparator, wake from Sleep.
* Tests: `crates/picsim/tests/cmp.rs`.

## Board parts: LEDs and an SSD1306 OLED (`periph/board.rs`, stimulus `[[led]]` / `[[oled]]`, GUI "Board" tab)

* LED: lit while its pin is an output (TRIS clear) at the active level (`active = "high"` or `"low"`); an input pin leaves it dark.
  The share of cycles it is lit gives a brightness (PWM/multiplexed LEDs); every change is kept with its cycle (`basil-sim` prints
  `led NAME (RB0, active high): ON, on N of M cycles, K changes: 1@123 0@456 ...`).
* OLED (SSD1306, 128 x 64 or 128 x 32, address default 0x3C): the pins are chosen in the stimulus file (`scl`, `sda`) or in the GUI
  and the program must use the same ones.
  * `bus = "i2c1"`/`"i2c2"` (hardware I2C host): the OLED acknowledges only when `SSPxCLKPPS`/`SSPxDATPPS` (Q41: `I2CxSCLPPS`/
    `I2CxSDAPPS`) select its SCL/SDA pins and `RxyPPS` of both pins holds the bus output code: Q10 MSSP1 SCL 0x0F, SDA 0x10,
    MSSP2 0x11/0x12 (DS40002043F Table 18-2 p.287, "PPS input and output select the same pin" 18.3); Q41 I2C1 SCL 0x21, SDA 0x22
    (DS40002214G Table 21-2 p.323). Otherwise the address gets a NACK and `status` names the register that is wrong.
  * `bus = "gpio"` (bit-banged): the two pins are watched after every instruction. Start/Stop are SDA edges while SCL is high, a bit
    is SDA at the rising SCL edge, the OLED pulls SDA low for the acknowledge (and for the bits of a read, which returns 0). A line is
    low when the PIC drives it low (TRIS clear and LAT clear) or the OLED holds it; external pull-ups are assumed, so a released
    pin reads 1 (an input pin with no OLED on it still reads 0 until driven, as before).
  * Commands (SSD1306 data sheet Rev 1.1, `docs/pdf/SSD1306-Rev1.1.pdf`, Table 9-1 p.28-32): display on/off, contrast, addressing mode
    0/1/2 with the pointer rules of 10.1.3 (p.34-35; page mode returns to the column start address, not 0), column/page range, page
    and column start, start line, display offset and multiplex ratio (Tables 10-1..10-3 p.37-38: COM c shows line c + offset and RAM
    row line + start line, modulo the ratio), segment remap (applies to data written afterwards, 10.1.8), COM scan direction, invert,
    entire display on, charge pump, scrolling (below).
  * Charge pump (application note p.3, 8Dh A[2]; reset = disabled): the panel is lit only with the pump enabled or `external_vcc = true`
    on the `[[oled]]` (modules with their own VCC); display on without either gives a note and a dark panel (the GUI shows why and has the
    checkbox). The pump setting counts whenever it is made; the documented order 8Dh 14h AFh is not required `[A65]`.
  * Scrolling (Table 9-1 p.28-29, 10.2 p.44-46): 26h/27h shift pages start..end right/left one column per step (SEG127 wraps to SEG0),
    29h/2Ah add E rows per step inside the A3h scroll area (rows move down, the last wraps to the first, A3h note 4 `[A67]`). The RAM is
    not changed, the picture moves. A step is C[2:0] frames (5, 64, 128, 256, 3, 4, 25, 2); a frame is D x K x MUX display clocks at Fosc
    (8.3 p.22), D from D5h, K = phase 1 + phase 2 + 50 from D9h (54 at reset), MUX from A8h. `[A66]` Fosc is 370 kHz (Table 13-1 typ) whatever
    D5h A[7:4] says: the data sheet gives no table for that field. At reset a step of 5 frames is about 47 ms. Time comes from the PIC
    clock: `fosc_mhz` in the stimulus file (default 64) or the GUI's Fosc box (`Board::fosc_mhz`). 2Eh stops it and the picture returns to the RAM
    (a note repeats 10.2.3: the RAM must be rewritten); a note is also printed for 2Fh with no setup, an end page below the start page
    (ignored), a setup while scrolling and RAM writes while scrolling (10.2.1, 10.2.2, 10.2.4).
  * Reads: "Under serial mode, only write operations are allowed" (PDF p.17), "No data read is provided in serial mode operation"
    (9.1, p.33) and "No status read is provided for serial mode" (10.1.21, p.43). The status register of Table 9-2 therefore does not
    exist on I2C. The OLED acknowledges the read address (8.1.5 a: it responds to the address including R/W#) and drives nothing: the
    host reads 0xFF and a note is printed.
* `[A64]` The data sheet defines no behaviour for the data phase after a read address (only that read mode exists); acknowledging the
  address and leaving SDA high is the model's choice. The 8Dh charge pump command is from the application note at the end of the same PDF.
* `[A65]` Charge pump order not enforced; `[A66]` Fosc fixed at 370 kHz; `[A67]` vertical scroll direction read from A3h note 4 (the data sheet shows it only in Figure 10-10).
* The GUI Board panel is shown before a program is loaded (pins of the selected device) and the wiring is applied on Load.
* Pin changes in the GUI re-attach the part: the OLED starts blank, like a power-up. Reset keeps the wiring.
* Tests: `crates/picsim/tests/board.rs` (hardware bus through the MSSP model, bit-banged lines driven by the test, wrong-pin cases).

## WWDT and resets (DS40002043F ch.10 p.96-106, 9.5-9.12 p.88-90; `periph/wdt.rs`, `Pic18::reset_cond`, `tests/wdt.rs`)

* Modes by WDTE (Table 10-1): 3 on, 2 on while awake, 1 on with WDTCON0.SEN, 0 off. Period 2^(WDTPS+5) clocks of 31 kHz LFINTOSC (WDTCS = 000) or 31.25 kHz
  MFINTOSC (001); WDTPS 19-31 give 2^5. WDTCPS, WDTCWS, WDTCCS other than all-ones make WDTPS, WINDOW, WDTCS read only. WDTCON0 reset value PS = 01011 (2 s).
* Window: WDTTMR[7:3] counts 32nds of the period; WINDOW 110..000 close the first 4, 8, 12, 16, 20, 24, 28 counts; 111 is not windowed. In windowed mode a CLRWDT
  needs the WWDT armed (read of WDTCON0; WDTTMR.STATE) and an open window, otherwise it is a window violation. Cleared by a valid CLRWDT, by being off, by
  any write to WDTCON0/WDTCON1 and by any reset.
* Table 9-3 (p.89-90) for the resets: WDT time-out: STATUS.nTO = 0, PCON0.RWDT = 0; window violation: PCON0.WDTWV = 0 (STATUS unchanged); RESET instruction: PCON0.RI = 0.
  Registers take the MCLR column of the pack (`u`, `q`, `x` bits unchanged); configuration-dependent ones (OSCCON/OSCFRQ/WDTCON) are set again from the configuration words.
  PC = reset vector, data RAM and program stay.
* `[A88]` The time base is exactly 31 kHz (31.25 kHz) with no tolerance; Fosc comes from the stimulus `fosc_mhz` (default 64): 516.13 cycles per clock at 64 MHz. The watchdog is
  looked at after every instruction, so the reset happens at the end of the instruction in which the deadline passes.
* `[A89]` PSCNT (WDTPSL/WDTPSH/WDTTMR[1:0]) = elapsed clocks modulo 2^WDTPS (debug registers; the data sheet gives no more).
* `[A90]` A valid CLRWDT in windowed mode disarms the WWDT again; the read of WDTCON0 arms it in every mode; a write to WDTCON0/1 disarms it.
* `[A91]` The table, not the prose of 9.5 (TO, PD and RWDT "change"), decides which bits a reset changes: a time-out clears nTO and RWDT, a violation only WDTWV. PD is not touched.
* `[A92]` After a reset other than the power-on reset the peripherals are rebuilt (all their state starts over) and the stimulus is applied again from its start (a `[[uart]]` stream
  or I2C client begins again; pin and analog levels are by absolute cycle and stay right). The logs (`tx`, `pin_log`, ...), the notes and the board stay. The reset takes no time.
* `[A93]` Sleep (DS40002043F 7.2.1 p.69, 10.6 p.99, Table 9-3 p.90): SLEEP sets TO = 1, PD = 0 and clears the WWDT; with an enabled interrupt flag already pending (PEIE for groups after PIR0, GIE ignored) it is a NOP.
  The WWDT counts in Sleep for WDTE = 3, or 1 with SEN; WDTE = 2 is off. A time-out wakes the part with no reset: PC + 2, TO = PD = 0, PCON0 unchanged (the table, as `[A91]`). An enabled interrupt flag wakes it
  with TO = 1, PD = 0 (the OV = 0 in the table row is not modelled); with GIE set the vector is taken after the instruction following SLEEP. The WWDT is cleared again on wake. With no interrupt enabled the
  run jumps to the time-out; with nothing that can wake the part `Stop::Sleep` ends the run. No oscillator start-up delay; brown-out resets are not modelled (stack resets: `[A99]`, MCLR: `[A100]`). What the peripherals do meanwhile is `[A94]`.
* `[A94]` Peripherals in Sleep (DS40002043F 7.2 p.68, 7.4 p.71: Fosc and Fosc/4 are off, every other clock source runs; the Q41 says the same, DS40002214G 34.19 p.545, 36.3.15 p.621). Everything the model clocks
  from Fosc or Fosc/4 has no clock: Timer0 (synchronous, 19.4 p.298), Timer1/3/5 (Fosc/4 and Fosc, 22.2.5 p.348; on a pin, LFINTOSC or SOSC with SYNC = 1 they go on, `[A102]`), Timer2/4/6 (21.7 p.337) and so CCP compare, capture and PWM edges, the EUSART and UART
  (29.4 p.523; Q41 34.19), the MSSP and SPI hosts and the I2C hosts (28.2.5 p.450; Q41 35.5 p.586, 36.3.15 p.621), and a running ADC conversion (33.2.4 p.552, only the FRC clock works in Sleep and it is not modelled).
  They stop where they are and go on from there at the wake-up: every cycle they have in hand moves later by the length of the Sleep (`Periph::thaw`). Pins keep their levels (22.4.7 p.354).
  Still running: the watchdog (`[A93]`), the comparators, the CLCs, the CWG dead band (`[A73]`: counted in Fosc, which a CS = 0 CWG does not have in Sleep, 25.4 p.389; the model does not stop it) and the data EEPROM
  write (its clock is not stated; it is not stopped). Idle (CPUDOZE IDLEN = 1, 7.3 p.71) keeps every peripheral clock running; the rest of Idle (PD/TO, WWDT wake) is as for Sleep.
  Assumptions: a character the stimulus sends to a UART in Sleep, or being received when SLEEP executes, is lost (no clock to sample it); an edge on a capture pin in Sleep is ignored (a Timer1 that does count in Sleep, `[A102]`, captures nothing);
  a conversion in progress at SLEEP finishes after the wake-up. A Sleep with an enabled interrupt that nothing can raise (its timer has no clock) goes on until the watchdog or the end of the run; `sleep_dead` does not look at that.
* `[A102]` Timer0 and Timer1/3/5 on the clocks that run in Sleep (DS40002043F Table 19-1 p.296, 19.2.2-19.2.3 and 19.4 p.296-298, Table 20-2 p.306, 20.5 p.308, 20.9 p.314, OSCEN p.60, 7.4 p.71; Q41: T0CON1 p.360, Table 25-4 p.377).
  Sources modelled: the T0CKIPPS / TxCKIPPS pin (Timer0 CS = 000, or 001 inverted: falling edges; Timer1 CS = 00000), LFINTOSC 31 kHz (Timer0 100, Timer1 00100) and SOSC 32.768 kHz (Q10: Timer0 101, Timer1 00110; Q41: Timer0 110, Timer1 00111). Each counts awake as well (the
  edges by absolute time: edge `k` falls on cycle `ceil(k * Fosc / (4 * f))`, Fosc from the board's `fosc_mhz`, 64 MHz until set; pin edges are the stimulus changes of the pin). Not modelled, they do not count and a note says so: HFINTOSC, MFINTOSC, EXTOSC, CLKREF, CLCx_out, the timer overflow sources.
  In Sleep a timer goes on when it is ON and its input is asynchronous (Timer0 T0ASYNC = 1, Timer1 SYNC = 1) on one of those sources; otherwise it halts like a Fosc/4 timer and `thaw` moves it. The decision is taken at SLEEP and kept for that Sleep (`sleep_run`). The flag is set on the cycle of the edge that
  overflows or matches, and an enabled flag wakes the part there (PEIE for PIR1 and up); a CCP compare on such a Timer1 works in Sleep. An oscillator timer needs no OSCEN bit and no start-up time: selecting the source is the peripheral request that 7.4 p.71 counts ("enabled manually or
  through peripheral clock selection before the part enters Sleep"); SOSCEN / LFOEN, OSCSTAT.SOR / LFOR, SOSCPWR and the 20.4 start-up delay are not modelled. The synchroniser of SYNC = 0 / T0ASYNC = 0 awake adds no delay. The Timer1 rule that a falling edge must precede the first counted
  rising edge (20.2.2 p.307) is not modelled (every rising edge after the timer is on counts). Pin edges made by the program (an output latch) or by a peripheral are not seen, as `[A101]`. Idle keeps all of this running as it does Fosc/4. To keep a long Sleep cheap the run jumps from edge to edge of the
  oscillator (and to the watchdog time-out, a stimulus change, a scheduled pin change or the end of an EEPROM write) while an interrupt is enabled; with no interrupt enabled it still jumps to the watchdog time-out. Tests: `tests/timer_sleep.rs`.

## Q10 CRC and memory scanner (DS40002043F ch.14 p.174-192; `periph/crc.rs`, `tests/crc.rs`)

Facts (data sheet):
* CRCCON0 EN/GO/BUSY/ACCM/SHIFTM/FULL, CRCCON1 DLEN[7:4] and PLEN[3:0], CRCDAT, CRCACC, CRCSHIFT, CRCXOR (bit 0 reads 1, polynomial LSb always 1) (14.12 p.182-187). The shifter starts on a write to CRCDATL (14.4.1 p.176); FULL = data waits for the shifter; CRCIF is set when BUSY goes 1 -> 0 (14.6 p.177).
* Figure 14-1 p.175 (data 55 66 77 88, x16+x15+x2+1, seed 0, DLEN 7, PLEN 15, ACCM = 1) gives 0x32D6 (SHIFTM = 0) and 0x6BA2 (SHIFTM = 1). These are the direct CRC algorithm, equal to the augmented shifter followed by PLEN+1 zeros. ACCM = 0 is the augmented shifter without the zeros; entering PLEN+1 zeros afterwards gives the ACCM = 1 value (14.5 p.176). Unit tests in `crc.rs`.
* Scanner (14.8-14.10 p.178-179, SCANCON0 p.188-189, SCANLADR/HADR p.190-191): SCANGO needs SCANEN and the CRC EN and GO, else INVALID = 1 and no scan; LADR > HADR clears SCANGO; the scan ends at HADR or an unimplemented address with SCANIF; SCANLADR/HADR writes are ignored while SCANGO = 1; Burst stalls the CPU until the end, Concurrent shares memory cycles, Peek uses dead cycles only, Triggered waits for a rising edge of the SCANTRIG source; INTM = 1 pauses Concurrent/Triggered during an interrupt. SCANE (CONFIG4) and SCANMD/CRCMD (PMD0) switch the modules off.

Assumptions (the data sheet is silent):
* `[A95]` The shifter takes one instruction cycle per data bit (DLEN + 1 cycles per word). A word written while the shifter is busy is held (FULL = 1) and starts when the shifter frees; a third word replaces the held one `[A96]`. With GO = 0 nothing shifts (data wait, BUSY = 1); a word already shifting finishes. Clearing EN abandons the shifter and the held word (no CRCIF). The shifter has no clock in Sleep. CRCSHIFT shows the word with the consumed bits shifted out.
* `[A97]` INVALID is cleared on a valid start and set when SCANGO is refused, when the CRC stops under a scan, and when the address rolls over past program memory. A scan that ends is flagged with SCANIF; SCANGO cleared by software also sets SCANIF (14.9: 1 -> 0 transition).
* `[A98]` The scanner fetches one program byte per access (SCANLADR + 1 each time; the data sheet does not give the width) and hands it to the CRC when the CRC can take it. Burst: each fetch costs one cycle and the CPU waits for the shifter; the stall ends when the last byte is handed over (the shifter finishes after). Concurrent and Triggered: after every instruction the scanner fills the CRC and each fetch steals one cycle. Peek: one fetch per dead cycle (cycles of an instruction beyond the first). INTM = 1 in Burst is not modelled (note). Trigger sources modelled: CLC1-CLC8 only (TSEL 16-23); the others never start the scan (note). The first trigger level seen when the scan starts is the previous level, not an edge. SCANBUSY mirrors "running and the CRC can take a word".
* `[A99]` Stack faults (DS40002043F 11.1.2.2-11.1.2.3 p.111, Table 9-3 p.90; DS40002214G 9.1.3.2 p.61). STKOVF (PCON0 bit 7) is set by a push onto a full stack (the 32nd on the Q10, the 128th on the Q41), STKUNF (bit 6) by a pop from an empty one, which returns 0; both bits are set whether or not STVREN is. With STVREN = 1 (default, also when the part has no such bit) the part resets at the end of that step (the instruction or the interrupt entry that did it): PC to the reset vector, STKPTR 0, the flag stays, other PCON0 bits unchanged. With STVREN = 0 a full stack keeps STKPTR and the new address overwrites the top entry, and an empty stack only returns 0. STKPTR bits 7:6 are not used (the older PIC18 STKFUL/STKUNF in STKPTR do not exist on these parts).
* `[A100]` MCLR pin (DS40002043F 9.4 p.87-88, 9.10 p.88, Table 9-3 p.89, CONFIG2 p.32). The stimulus `[[mclr]]` (`level`, `at`; high until driven) drives the pin. A falling edge is a reset only while the pin is the reset input: LVP = 1, or MCLRE = 1 (otherwise the pin is an ordinary input and nothing happens). Reset: as `[A91]`, RMCLR (PCON0 bit 3) cleared, STATUS untouched (so a reset in Sleep keeps TO = 1, PD = 0 as SLEEP left them, which is the table's -10u), the part wakes. While the pin stays low the part is held (time passes, nothing runs, peripherals stay in their reset state); it runs again at the rising edge. The 10 Fosc delay after the release (p.88-89), the minimum pulse width (TMCLR) and the oscillator start-up are not modelled. A pulse that starts and ends inside one step still resets. The pin level in PORTE.RE3 is not modelled.
* `[A101]` Interrupt-on-change and INT0-INT2 (DS40002043F 17.4-17.8 p.271, 15.8 p.195, 15.10 p.195, INTCON p.198). Edges are looked for on the pin levels the stimulus produces (`[[pin]]`, `[[port]]`) after every instruction and also in Sleep: a pin whose IOCxP bit is set and rises, or whose IOCxN bit is set and falls, sets its IOCxF bit whatever IOCIE says; IOCIF in PIR0 is rewritten from the IOCxF registers after every step (so it is read-only and follows a software clear). INTx: the pin of INTxPPS, the edge of INTxEDG, sets INTxIF whatever INTxIE says. In Sleep the pending flag with its enable wakes the part through the generic rule `[A93]` (INTxIE, IOCIE; PEIE not needed in PIE0). The Sleep run stops at every stimulus change so the wake-up is at the edge. Not modelled: edges caused by software (an output pin whose latch changes) or by peripherals driving the pin; the rule that an edge seen while IOCxF is being cleared sets the flag again; the Q41 vector interrupt controller's own INTx handling beyond the flag. Tests: `tests/ioc.rs` (Q10).
* Not modelled: ICD interaction (Table 14-3), WWDT is not paused by a scan (as the data sheet says).
* `[A103]` Implemented masks of SFRs in the device descriptions, corrected against the data sheets (tests: `tests/q41_irq_masks.rs`). They had dropped real bits, so software writes were lost. Q41 (DS40002214G): `PIR2`/`PIE2`/`IPR2` 0xF1 (p.148), `PIR0`/`PIE0`/`IPR0` 0xFF (bit 5 CLC1IF/IE/IP, p.144/133/163), `PMD0` 0xFB (p.293), `PMD1`/`PMD2`/`PMD4` 0xFF (p.294-297), `WDTPSL`/`WDTPSH` 0xFF, `MD1CARH` 0x0F, `DAC1DATL`/`DAC2DATL` 0xFF, `CLCDATA` 0x0F. Q10 (DS40002043F): `PMD0`/`PMD3` 0xFF, `PMD2` 0x67, `PMD4`/`PMD5` 0xF1 (p.77-83), `PIE3`/`IPR3` 0xFF (p.210, 218; the old masks left TX2/RC2/SSP2/BCL2 unusable). 16F131xx/152xx/171x6 (DS40002519D, DS40002195E, DS40002343F): `WDTPSL`/`WDTPSH` 0xFF, `PORTC` 0x3F on the 14-pin parts and 0xFF on the 20-pin parts (pin allocation tables). `OUT` of CCPxCON and PWMxCON (DS40002043F 23.12.1 p.366) reads the live output level (after POL); MDB does not model it, so tests that read those registers mask bit 5. Not audited: the other registers whose mask is narrower than their declared fields (many are legitimate: selector fields declared 8 bits wide), and reset values (the Q41 `IPR0` reset is listed as 00000000, the data sheet says 10111111).

## PIC16 enhanced mid-range core (S2, `pic16.rs`, `machine16.rs`, `tests/pic16_core.rs`; PIC16F17126/46, DS40002343F)

Core only: no peripheral is modelled yet, so their registers are plain storage and `basil-sim` prints "unmodelled SFRs touched". `Machine` (GUI, `Stimulus`, peripherals) is still PIC18 only; `basil-sim` picks `Machine16` for `family = "pic16e"`. Symbols from `;SYM`/`equ` are word addresses for code and data addresses for variables; the PC is in words.

* Instruction set: Table 41-3 p.592-594, descriptions p.595-607. 50 instructions, 14-bit words, byte `2w`/`2w+1` of the hex file. `GOTO`/`CALL`: PC[10:0] = k, PC[14:11] = PCLATH[6:3] (p.597, 599; the GOTO text says PCLATH[4:3], the CALL text and 9.4.3 p.52 say [6:3]; the model follows the latter). `CALLW`: PCLATH[6:0]:W. `BRA`: PC + 1 + k (9 bits signed), `BRW`: PC + 1 + W. `ADDFSR` k is 6 bits signed. `MOVIW`/`MOVWI` mm: 00 `++FSR`, 01 `--FSR`, 10 `FSR++`, 11 `FSR--`; `MOVIW` sets Z (p.601, 603). `SUBWFB` is f - W - NOT C. `SUBWF`/`SUBLW`: C = 1 when no borrow (p.606).
* Cycles (p.591): 1; 2 for CALL, CALLW, RETURN, RETLW, RETFIE, GOTO, BRA, BRW, a taken skip; +1 when an instruction reaches program flash through INDFn (p.591 note 2, 9.6.3 p.57). A write to PCL is taken as 2 cycles (the PC changes) `[A104]`.
* Data memory: 64 banks of 128 bytes; offsets 00h-0Bh (core registers) and 70h-7Fh (common RAM) are the same bytes in every bank (9.2 p.42-43, Fig. 9-5.. p.44-46). `FSRn`: 0000h-1FFFh banked, 2000h-2FEFh linear (80-byte GPR blocks of banks 0-50 end to end, 9.6.2 p.57), bit 15 set = program flash word (low 8 bits, reads only, 9.6.3 p.57), the rest reads 0. An `INDF` that points at an `INDF` reads 0 and ignores writes (9.6 p.55).
* STATUS (9.3 p.50): when the destination is STATUS and the instruction sets Z, DC or C, those three bits are not written; TO and PD are never written. `CLRF STATUS` leaves C and DC and sets Z (p.50-51).
* Stack (9.5 p.52-55): 16 levels of 15 bits, STKPTR 5 bits resets to 1Fh (empty). The 17th push gives pointer 10h, sets STKOVF and overwrites entry 0, or resets the part when STVREN = 1 (the push is not done). A pop of an empty stack sets STKUNF and resets the part when STVREN = 1. TOSL/TOSH read the entry STKPTR points at; with an empty stack 0 if STVREN = 1, entry 0Fh if not (Fig. 9-14 p.53); writes with an empty stack are dropped.
* Interrupts (12.5 p.110-111, 12.9 p.113, Fig. 12-1 p.110): taken when GIE = 1 and a PIEn & PIRn bit is set, for group 0 (PIE0/PIR0) always and for groups 1-7 only with PEIE = 1. Entry: GIE cleared, PC pushed, W, STATUS (not TO/PD), BSR, FSR0/1 and PCLATH saved in `*_SHAD` (bank 63, readable and writable), PC = 0004h. `RETFIE` pops, restores them and sets GIE. SLEEP: PD = 0, TO = 1; any enabled PIE & PIR pair wakes it (GIE and PEIE not needed); the instruction after SLEEP runs before an interrupt (12.7 p.113).
* Configuration words are 14-bit words at word addresses 8007h-800Bh (bytes 1000Eh.. in the hex); `Config::decode_words`.
* The `RESET` instruction clears PCON0.nRI (p.603-604); every reset loads the MCLR column of the SFRs and empties the stack.

Assumptions (the data sheet is silent):
* `[A104]` A write to PCL costs 2 cycles; an interrupt costs 2 cycles beyond the interrupted instruction (the two forced NOPs of Fig. 12-2 p.112, not the "3-5 TCY" latency, which includes the time the event takes to be sampled). A request that is already pending when `BSF INTCON,GIE` or `RETFIE` sets GIE is taken at once (MDB agrees: `tests/probe/retfie_pending.S`, vector 2 cycles after the instruction, stacked PC = the instruction at the return address). A request that arrives while the program runs is `[A108]`.
* `[A105]` Pushes beyond the 17th with STVREN = 0 keep counting STKPTR (up to the empty marker 1Fh); the data sheet describes only the 17th. Stack underflow with STVREN = 0 returns entry 0Fh and leaves STKPTR at 1Fh; the data sheet does not say what the pointer does.
* `[A106]` Timer0 on the PIC16 core (DS40002343F 22.1-22.5 p.244-249, T0CON0 p.247, T0CON1 p.248, TMR0H p.249; same behaviour as the Q10 model, `[A1]`-`[A3]`). Only the Fosc/4 clock (CS = 010) counts; other sources log a note and do not count. It does not count in Sleep (Fosc/4 stops). A register access brings the timer to the cycle of the accessing instruction (`[A2]`: a write takes effect after that cycle; a TMR0L read sees the count through it); the timer is also brought up to date at the end of every step, so the flag is set before the next interrupt check. The TMR0IF interrupt then goes through the existing PIE0/PIR0 path (`[A104]`, `[A108]` entry timing; flag time agrees with MDB when polled, `tests/probe/t0_poll.S`, and `isr_single` gives MDB's variables and cycle count). A reset (RESET, stack fault) clears count, prescaler and postscaler; ASYNC, the pin, the PPS input and the CLC source are not modelled.
* `[A107]` Timer1/3 and Timer2/4/6 on the PIC16 core (DS40002343F ch.23 p.252-267, ch.24 p.268-287; same behaviour as the Q10 model). Timer1: 16-bit, prescaler 1:1-1:8, TMRxIF on FFFFh -> 0000h, RD16 buffers TMRxH, only a TMRxL write clears the prescaler (23.6); clocks Fosc/4 (CS = 1) and Fosc (CS = 2, four ticks a cycle) only. GE = 1 (gate) is not modelled: the timer holds and a note is logged. Timer2: T2TMR counts to T2PR and goes to 0 on the next clock, postscaler OUTPS+1 matches per TMRxIF, prescaler/postscaler cleared by a write to T2TMR or T2CON (T2TMR is not, 24.1); CS = 1 (Fosc/4) and 2 (Fosc); MODE 00000 (software gate) and 01000 (software-start one-shot, clears ON at the match); other MODEs run as the software mode with a note (no external reset sources). The count is kept when ON is cleared (HLT note 6). No timer counts in Sleep. Resets: Timer1 keeps its count (MCLR column u), T2TMR clears and T2PR becomes FFh (MCLR column). The two-instruction-cycle flag synchronisation of Fig. 24-2 is not modelled; PSYNC/CSYNC/CPOL, TMRxGIF, the CCP special event trigger and PWM use are not either. Not yet checked against MDB.
* `[A108]` Interrupt entry when the request arrives while the program is running (measured against MDB with `tests/probe/int_first.S`: the first interrupt cutting a chain of 1-cycle or of 2-cycle instructions, six flag phases each: cycle and stacked PC identical; `isr_single`: all 300 entries and the total cycle count identical). The instruction already fetched at the return address is executed first (Fig. 12-2 p.112: Execute PC, then the two NOPs; note 2 p.113: the latency is the same for 1- and 2-cycle instructions); the stacked PC is the one after it, and the vector is reached 3 cycles after that instruction started (2 cycles after a 1-cycle instruction, 1 after a 2-cycle one). A request that was already set at the start of the previous step is `[A104]`. Not matched yet: back-to-back interrupts where the request arrives in the last cycles of the handler and the main program has 2-cycle instructions (`tests/probe/retfie_phase.S`: later entries differ by 1-2 cycles for most periods; MDB keeps the entry spacing equal to the timer period there). A fresh request whose instruction clears GIE or PIE drops the interrupt. Not measured: a request made pending by software (`BSF PIE0,x` with the flag already set, `BSF PIR0,x`, PEIE) counts as arriving while running here, which MDB may treat as `[A104]`; a flag cleared and set again inside one step is not seen as new.
* Not modelled: the watchdog (CLRWDT only sets TO and PD), oscillator and `RSTOSC` reset values, all peripherals but the timers, the PIC16F183xx (`Pic16::new` refuses a part whose BSR is not 6 bits wide: 5-bit BSR and INTCON-style interrupt registers; no 183xx data sheet is on disk to check its instruction encodings), ELF debug information (the compiler emits none for the enhanced mid-range core yet, so `run16` needs the `.S` listing), the Memory Execution Violation reset, NVM/EEPROM/flash writes.

Status: `Basic_Compiler/scripts/exec-test.py --device 16F17146 --engine both` (run from the Basic_Compiler repo) on its `tests/exec_p16` (addressof_byref, addressof_table, delay_1ms, delay_50, delay_us, delay_var, float_random, lib_i2c, lib_pwm, lib_spi) gives MDB's variables and cycle counts exactly. `lib_i2c`, `lib_pwm` and `lib_spi` pass because registers are plain storage, as in MDB-compatible register-protocol tests; they say nothing about the peripherals. `isr_single` also passes now (Timer0/Timer2 interrupts, 262004 cycles, same as MDB). The other five (lib_adc, lib_eeprom, lib_flashwrite, lib_serial, lib_serial2) need the ADC, NVM and the EUSART (S3-S5) and fail.
PIC16F18056/76 (device files copied from Basic_Compiler 0.1.152; same `Pic16` core, DS40002325C not re-read for the core): the register-only `Basic_Compiler/tests/exec_p180` programs (lib_i2c, lib_intosc, lib_spi) halt; the others need Timer0/1/2, the ADC, the EUSART or the oscillator registers (`lib_system` reads OSCSTAT-dependent bits). `float_compare`, `lib_floatlib_edge`, `arith_integer` and `arith_word`, which MDB gets wrong on these parts (it ignores writes to 5A0h-5A3h), give the expected results here with identical cycle counts on 16F18056, 16F18076 and 16F17146 (`Basic_Compiler/scripts/exec-test.py --engine sim`; in this repo `tests/pic16_core.rs` only runs a four-instruction smoke program on each of 16F17126/46, 16F18056/76, 16F13145 and 16F15244).
