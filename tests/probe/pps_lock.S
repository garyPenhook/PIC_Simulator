; Probe: PPSLOCK sequence. RC0PPS is set to 4, then MODE picks what is done to PPSLOCK; RC0PPS is then written 7. PPSLOCK
; goes to 20h and RC0PPS to 21h. ONEWAY = 0 clears the PPS1WAY configuration bit (the unprogrammed value is 1).
; MODE 0 write 1 to PPSLOCK without the sequence; 1 lock sequence; 2 lock sequence, then unlock sequence; 3 55h, other
; value, AAh, BSF; 4 55h, MOVWF to another register, AAh, BSF; 5 lock, unlock, lock again, unlock; 6 55h, AAh, then two
; writes of 1 (the second without a new sequence); 7 AAh, 55h, AAh, BSF.
#include <xc.inc>
#if ONEWAY == 0
config PPS1WAY = OFF
#endif
PSECT resetVec,class=CODE,reloc=2,delta=2
    goto start
PSECT code,class=CODE,reloc=2,delta=2
start:
    banksel(RC0PPS)
    movlw 4
    movwf RC0PPS&0x7F
    banksel(PPSLOCK)
#if MODE == 0
    movlw 1
    movwf PPSLOCK&0x7F
#elif MODE == 1
    movlw 0x55
    movwf PPSLOCK&0x7F
    movlw 0xAA
    movwf PPSLOCK&0x7F
    bsf PPSLOCK&0x7F,0
#elif MODE == 2 || MODE == 5
    movlw 0x55
    movwf PPSLOCK&0x7F
    movlw 0xAA
    movwf PPSLOCK&0x7F
    bsf PPSLOCK&0x7F,0
    movlw 0x55
    movwf PPSLOCK&0x7F
    movlw 0xAA
    movwf PPSLOCK&0x7F
    bcf PPSLOCK&0x7F,0
#if MODE == 5
    movlw 0x55
    movwf PPSLOCK&0x7F
    movlw 0xAA
    movwf PPSLOCK&0x7F
    bsf PPSLOCK&0x7F,0
#endif
#elif MODE == 3
    movlw 0x55
    movwf PPSLOCK&0x7F
    movlw 0x12
    movwf PPSLOCK&0x7F
    movlw 0xAA
    movwf PPSLOCK&0x7F
    bsf PPSLOCK&0x7F,0
#elif MODE == 4
    movlw 0x55
    movwf PPSLOCK&0x7F
    movwf 0x70
    movlw 0xAA
    movwf PPSLOCK&0x7F
    bsf PPSLOCK&0x7F,0
#elif MODE == 6
    movlw 0x55
    movwf PPSLOCK&0x7F
    movlw 0xAA
    movwf PPSLOCK&0x7F
    movlw 1
    movwf PPSLOCK&0x7F
    bcf PPSLOCK&0x7F,0
    bsf PPSLOCK&0x7F,0
#elif MODE == 7
    movlw 0xAA
    movwf PPSLOCK&0x7F
    movlw 0x55
    movwf PPSLOCK&0x7F
    movlw 0xAA
    movwf PPSLOCK&0x7F
    bsf PPSLOCK&0x7F,0
#endif
    movf PPSLOCK&0x7F,w
    movwf 0x70
    banksel(RC0PPS)
    movlw 7
    movwf RC0PPS&0x7F
    movf RC0PPS&0x7F,w
    movwf 0x71
    banksel(0x20)
    movf 0x70,w
    movwf 0x20
    movf 0x71,w
    movwf 0x21
done:
    nop
    goto done
