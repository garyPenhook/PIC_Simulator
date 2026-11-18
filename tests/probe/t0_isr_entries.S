; Probe (PIC18F27Q10), D-SIM-2: free-running Timer0 interrupts over a loop of 1-cycle and taken 2-cycle instructions.
; T1V = T0CON1 (40h 1:1, 41h 1:2), PERIOD = TMR0H, RW = 1: the handler writes TMR0H = PERIOD + (count & 3) like isr_single.
; The handler is `incf`, optional TMR0H rewrite, `bcf PIR0,5`, `retfie f`. Run in MDB and basil-sim with a breakpoint at 0008h
; and compare the cycle of every entry (the stopwatch deltas of repeated `run`s).
#include <xc.inc>
PSECT resetVec,class=CODE,reloc=2
    goto start
PSECT intVec,class=CODE,reloc=2,abs
    org 8
isr:
    incf 0x20,f,a
#if RW
    movf 0x20,w,a
    andlw 3
    addlw PERIOD
    banksel(TMR0H)
    movwf BANKMASK(TMR0H),b
#endif
    banksel(PIR0)
    bcf BANKMASK(PIR0),5,b
    retfie f
PSECT code,class=CODE,reloc=2,abs
    org 0x100
start:
    clrf 0x20,a
    banksel(T0CON1)
    movlw T1V
    movwf BANKMASK(T0CON1),b
    banksel(TMR0H)
    movlw PERIOD
    movwf BANKMASK(TMR0H),b
    banksel(PIE0)
    bsf BANKMASK(PIE0),5,b
    banksel(T0CON0)
    movlw 0x80
    movwf BANKMASK(T0CON0),b
    movlb 0
    bsf INTCON,6,a
    bsf INTCON,7,a
loop:
    REPT 6
    nop
    bra $+4
    nop
    movff 0x30,0x31
    decfsz 0x32,f,a
    ENDM
    bra loop
