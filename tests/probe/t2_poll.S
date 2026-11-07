; Probe (PIC16F17146 and the other parts with T2HLT/T2CLKCON): Timer2 (T2CLKCON CS = 0001 Fosc/4, DS40002343F Table 24-3
; p.286; T2HLT = 0 free running; T2CON = 80h: ON, 1:1, 1:1) with T2PR = PERIOD started, then TMR2IF (PIR2 bit 2 on the
; PIC16F17146) polled; `done` is reached when the flag is seen. CLR = 1, 2, 3 put a BCF of TMR2IF, a CLRF PIR2 or a
; MOVWF PIR2 of 0 after the PRE NOPs (CLR = 0: none), to see whether a clear in the cycle the timer sets the flag wins.
; Compare the cycle count at `done` between basil-sim and MDB.
#include <xc.inc>
PSECT resetVec,class=CODE,reloc=2,delta=2
    goto start
PSECT code,class=CODE,reloc=2,delta=2
start:
    banksel(T2CLKCON)
    clrf T2HLT&0x7F
    movlw 1
    movwf T2CLKCON&0x7F
    clrf T2TMR&0x7F
    movlw PERIOD
    movwf T2PR&0x7F
    banksel(PIR2)
    clrf PIR2&0x7F
    banksel(T2CON)
    movlw 0x80
    movwf T2CON&0x7F
    banksel(PIR2)
    movlw 0
    REPT PRE
    nop
    ENDM
#if CLR == 1
    bcf PIR2&0x7F,2
#endif
#if CLR == 2
    clrf PIR2&0x7F
#endif
#if CLR == 3
    movwf PIR2&0x7F
#endif
poll:
    btfss PIR2&0x7F,2
    goto poll
done:
    nop
    goto done
END
