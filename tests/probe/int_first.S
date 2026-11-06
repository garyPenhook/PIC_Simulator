; Probe: the FIRST Timer0 interrupt only, cutting a chain of identical instructions (CHAIN=1: nop, CHAIN=2: 2-cycle
; `bra $+1`). PERIOD moves the flag cycle one step at a time. Read the cycle of the vector entry and the stacked
; return address (TOSL, one word per chain element) in basil-sim and MDB.
#include <xc.inc>
PSECT resetVec,class=CODE,reloc=2,delta=2
    goto start
PSECT intVec,class=CODE,reloc=2,delta=2
isr:
    goto isr
PSECT code,class=CODE,reloc=2,delta=2
start:
    movlb 0
    banksel(T0CON1)
    movlw 0x40
    movwf T0CON1&0x7F
    banksel(TMR0H)
    movlw PERIOD
    movwf TMR0H&0x7F
    banksel(PIE0)
    bsf PIE0&0x7F,5
    banksel(T0CON0)
    movlw 0x80
    movwf T0CON0&0x7F
    movlb 0
    bsf INTCON,6
    bsf INTCON,7
chain:
#if CHAIN==1
    REPT 40
    nop
    ENDM
#else
    REPT 20
    bra $+1
    ENDM
#endif
    goto chain
END
