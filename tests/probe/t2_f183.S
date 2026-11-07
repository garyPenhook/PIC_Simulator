; Probe (PIC16F183xx): the older Timer2 (DS40001839E ch.28: TMR2/PR2/T2CON, Fosc/4 only). T2CON = CON (OUTPS<6:3>,
; TMR2ON bit 2, CKPS<1:0>) with PR2 = PR. After DLY NOPs, SAMPLES samples are taken every two cycles into 20h-..: TMR2
; when SRC = 0, PIR1 (TMR2IF is bit 1) when SRC = 1. Then TMR2IF is polled and `done` is reached when it is seen
; (SAMPLES = 0 times the flag alone, to a 3-cycle poll). With CLR = 1 a BCF of TMR2IF follows the DLY NOPs, with CLR = 2
; a CLRF PIR1, with CLR = 3 a MOVWF PIR1 of 0 (its MOVLW 0 replaces the first of the DLY NOPs, so DLY >= 1).
; TMR2, PR2, T2CON and PIR1 are all in bank 0 on this part.
; Compare the samples and the cycle count at `done` between basil-sim and MDB.
#include <xc.inc>
PSECT resetVec,class=CODE,reloc=2,delta=2
    goto start
PSECT code,class=CODE,reloc=2,delta=2
start:
    movlb 0
    clrf TMR2
    movlw PR
    movwf PR2
    movlw 0x20
    movwf FSR0L
    clrf FSR0H
    clrf PIR1
    movlw CON
    movwf T2CON
#if CLR == 3
    movlw 0
    REPT DLY-1
    nop
    ENDM
#else
    REPT DLY
    nop
    ENDM
#endif
#if CLR == 1
    bcf PIR1,1              ; clear TMR2IF here, to see whether a set in the same cycle wins
#endif
#if CLR == 2
    clrf PIR1
#endif
#if CLR == 3
    movwf PIR1
#endif
    REPT SAMPLES
IF SRC
    movf PIR1,w
ELSE
    movf TMR2,w
ENDIF
    movwi FSR0++
    ENDM
poll:
    btfss PIR1,1
    goto poll
done:
    nop
    goto done
END
