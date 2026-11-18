; Probe: one ADC conversion on the PIC18F27Q10 (ADC2, DS40002043F ch.33). ADCLK = CLK, ADPRE = PRE, ADACQ = ACQ,
; ADCON0 = 84h (ON, right justified). GO is set and polled until it reads 0; `done` is reached then. The cycle count at
; `done` in basil-sim and MDB, for several CLK/PRE/ACQ, shows which term of the conversion time differs (D-SIM-18).
; Bytes at 20h: ADRESL, ADRESH.
#include <xc.inc>
PSECT resetVec,class=CODE,reloc=2
    goto start
PSECT code,class=CODE,reloc=2,abs
    org 0x100
start:
    banksel(ADCLK)
    movlw CLK
    movwf BANKMASK(ADCLK),b
    movlw PRE
    movwf BANKMASK(ADPRE),b
    movlw ACQ
    movwf BANKMASK(ADACQ),b
    movlw 0x84
    movwf BANKMASK(ADCON0),b
    nop
    nop
    bsf BANKMASK(ADCON0),0,b
poll:
    btfsc BANKMASK(ADCON0),0,b
    bra poll
    movff ADRESL,0x20
    movff ADRESH,0x21
done:
    nop
    bra done
