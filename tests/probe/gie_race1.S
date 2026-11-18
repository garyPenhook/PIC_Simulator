; Probe (PIC18F27Q10), D-SIM-2: ONE Timer0 interrupt (8-bit, Fosc/4, 1:1, flag PERIOD+1 cycles after the timer starts)
; racing `bcf INTCON,7` / NIN nops / `bsf INTCON,7`, preceded by PRE nops. The handler stops Timer0's interrupt
; (PIE0.TMR0IE = 0), counts the entry at 20h and saves TOSL (the stacked return address) at 21h. `done` follows 60 nops.
; Bytes at 20h: entries, TOSL. Cycles at `done`: the same whenever the interrupt is taken once, so TOSL tells where it landed.
#include <xc.inc>
PSECT resetVec,class=CODE,reloc=2
    goto start
PSECT intVec,class=CODE,reloc=2,abs
    org 8
isr:
    incf 0x20,f,a
#if ISRV==0
    movff TOSL,0x21
#endif
    banksel(PIE0)
    bcf BANKMASK(PIE0),5,b
    banksel(PIR0)
    bcf BANKMASK(PIR0),5,b
#if RF
    retfie f
#else
    retfie
#endif
PSECT code,class=CODE,reloc=2,abs
    org 0x100
start:
    clrf 0x20,a
    clrf 0x21,a
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
    REPT PRE
    nop
    ENDM
#if WITHCS
    bcf INTCON,7,a
#endif
    REPT NIN
    nop
    ENDM
#if WITHCS
    bsf INTCON,7,a
#endif
#if TWO==6
    REPT 30
    lfsr 0,0x100
    ENDM
#elif TWO==5
    REPT 30
    tstfsz 0x33,a
    nop
    ENDM
#elif TWO==4
    REPT 30
    movff 0x30,0x31
    ENDM
#elif TWO==3
    REPT 30
    goto $+6
    nop
    ENDM
#elif TWO==2
    REPT 30
    bra $+4
    nop
    ENDM
#elif TWO==1
    REPT 40
    bra $+2
    ENDM
#else
    REPT 60
    nop
    ENDM
#endif
done:
    nop
    bra done
