; Probe: one conversion of the simple 10-bit ADC (PIC16F18346 DS40001839E ch.22, PIC16F15244 DS40002195E ch.27).
; ADCON1 = CON1 (ADFM/FM bit 7, ADCS/CS bits 6:4), CHS = CHS, ON, then GAP NOPs and GO (bit 1); `done` is reached when
; GO reads 0. Compare the cycle count at `done` and the bytes at 20h (ADRESL, ADRESH, ADCON0) between basil-sim and MDB.
#include <xc.inc>
PSECT resetVec,class=CODE,reloc=2,delta=2
    goto start
PSECT code,class=CODE,reloc=2,delta=2
start:
    banksel(ADCON1)
    movlw CON1
    movwf ADCON1&0x7F
    movlw (CHS<<2)|1
    movwf ADCON0&0x7F
    REPT GAP
    nop
    ENDM
    bsf ADCON0&0x7F,1
poll:
    btfsc ADCON0&0x7F,1
    goto poll
    movf ADRESL&0x7F,w
    movwf 0x70
    movf ADRESH&0x7F,w
    movwf 0x71
    movf ADCON0&0x7F,w
    movwf 0x72
    banksel(0x20)
    movf 0x70,w
    movwf 0x20
    movf 0x71,w
    movwf 0x21
    movf 0x72,w
    movwf 0x22
done:
    nop
    goto done
