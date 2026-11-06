; Probe: Timer0 (Fosc/4, 8-bit, 1:1) raises TMR0IF every PERIOD+1 cycles while the handler runs for a fixed time, so
; the flag is set at a different point relative to RETFIE for each PERIOD. The main program is a loop of distinct 1- and 2-cycle instructions, so the stacked address shows where it was cut.
; Compare, per PERIOD, the cycle of each vector entry and the stacked return address in basil-sim and MDB.
#include <xc.inc>
n   equ 0x21
PSECT resetVec,class=CODE,reloc=2,delta=2
    goto start
PSECT intVec,class=CODE,reloc=2,delta=2
isr:
    banksel(PIR0)
    bcf PIR0&0x7F,5
    movlb 0
    incf n,f
    retfie
PSECT code,class=CODE,reloc=2,delta=2
start:
    movlb 0
    clrf n
    banksel(T0CON1)
    movlw 0x40
    movwf T0CON1&0x7F
    banksel(TMR0H)
    movlw PERIOD
    movwf TMR0H&0x7F
    banksel(T0CON0)
    movlw 0x80
    movwf T0CON0&0x7F
    banksel(PIE0)
    bsf PIE0&0x7F,5
    movlb 0
    bsf INTCON,6
    bsf INTCON,7
loop:
    nop
    bra b1
b1:
    nop
    bra b2
b2:
    nop
    nop
    goto loop
END
