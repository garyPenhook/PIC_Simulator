; Probe: one ADC conversion on the PIC16F17146 (ADCC, DS40002343F ch.33). ADCLK = CLK, ADACQ = ACQ (low byte), ADPRE = PRE
; (low byte), ADCON0 = CON (84h: ON, right justified, single ended; 86h differential); GAP NOPs between turning the ADC on and setting GO; `done` is reached when GO reads 0.
; basil-sim reaches `done`; MDB never does (its ADC on the PIC16F17146 does not finish a conversion: D-SIM-6), so this probe checks the
; simulator only. Bytes at 20h: ADRESL, ADRESH, ADCON0, PIR6.
#include <xc.inc>
PSECT resetVec,class=CODE,reloc=2,delta=2
    goto start
PSECT code,class=CODE,reloc=2,delta=2
start:
    banksel(ADCLK)
    movlw CLK
    movwf ADCLK&0x7F
    movlw ACQ
    movwf ADACQL&0x7F
    movlw PRE
    movwf ADPREL&0x7F
    movlw CON
    movwf ADCON0&0x7F
    REPT GAP
    nop
    ENDM
    bsf ADCON0&0x7F,0
poll:
    btfsc ADCON0&0x7F,0
    goto poll
    movf ADRESL&0x7F,w
    movwf 0x70
    movf ADRESH&0x7F,w
    movwf 0x71
    movf ADCON0&0x7F,w
    movwf 0x72
    banksel(PIR6)
    movf PIR6&0x7F,w
    movwf 0x73
    banksel(0x20)
    movf 0x70,w
    movwf 0x20
    movf 0x71,w
    movwf 0x21
    movf 0x72,w
    movwf 0x22
    movf 0x73,w
    movwf 0x23
done:
    nop
    goto done
