; Probe (PIC18F27Q10): Timer0 (8-bit, Fosc/4, 1:1, period PERIOD+1 cycles) interrupts race a critical section
; `bcf INTCON,7` ... `bsf INTCON,7` (NIN nops inside, NOUT nops after) repeated N times; D-SIM-2. The handler counts
; entries at 20h and clears TMR0IF. The cycle count at `done` and the entry count show whether the engines take the
; same interrupts. Bytes at 20h: entries, then the loop counter.
#include <xc.inc>
PSECT resetVec,class=CODE,reloc=2
    goto start
PSECT intVec,class=CODE,reloc=2,abs
    org 8
isr:
    incf 0x20,f,a
    banksel(PIR0)
    bcf BANKMASK(PIR0),5,b
    retfie f
PSECT code,class=CODE,reloc=2,abs
    org 0x100
start:
    clrf 0x20,a
    movlw N
    movwf 0x21,a
    banksel(T0CON1)
    movlw 0x40
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
    bcf INTCON,7,a
    REPT NIN
    nop
    ENDM
    bsf INTCON,7,a
    REPT NOUT
    nop
    ENDM
    decfsz 0x21,f,a
    bra loop
done:
    nop
    bra done
