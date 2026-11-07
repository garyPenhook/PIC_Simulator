; Probe: when does the simple 10-bit ADC (PIC16F18346 / PIC16F15244) clear GO? CON1 as in adc_simple.S; GO (bit 1 of ADCON0) is set,
; N NOPs follow, then ADCON0 is read. Bytes at 20h: ADCON0 as read (bit 1 = GO still set), ADRESL, ADRESH, the PIR flag byte
; (PIR1; ADIF is bit 0 on the PIC16F15244 and bit 6 on the PIC16F18346). The first N with GO clear is the conversion time in cycles.
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
    nop
    bsf ADCON0&0x7F,1
    REPT N
    nop
    ENDM
    movf ADCON0&0x7F,w
    movwf 0x70
    movf ADRESL&0x7F,w
    movwf 0x71
    movf ADRESH&0x7F,w
    movwf 0x72
    banksel(PIR1)
    movf PIR1&0x7F,w
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
