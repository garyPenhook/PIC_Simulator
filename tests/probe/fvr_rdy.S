; Probe: FVRRDY. FVRCON is written with VAL and read back to 20h. The PIC16F17146 and PIC16F13145 follow FVREN (bit 7); the
; PIC16F152xx (DS40002195E 26.3.1 p.316) has FVRRDY always 1 (MDB agrees); the PIC16F183xx data sheet says so too (Register 16-1 p.180) but MDB follows FVREN there.
#include <xc.inc>
PSECT resetVec,class=CODE,reloc=2,delta=2
    goto start
PSECT code,class=CODE,reloc=2,delta=2
start:
    banksel(FVRCON)
    movf FVRCON&0x7F,w
    movwf 0x70
    movlw VAL
    movwf FVRCON&0x7F
    movf FVRCON&0x7F,w
    movwf 0x71
    banksel(0x20)
    movf 0x70,w
    movwf 0x20
    movf 0x71,w
    movwf 0x21
done:
    nop
    goto done
