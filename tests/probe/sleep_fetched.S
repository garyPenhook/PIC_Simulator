; Probe: SLEEP near a Timer0 request that arrives while running (int_first.S with the chain broken by a SLEEP after
; SLEEPAT nops; build with PERIOD=10). Read the vector cycle, TOSL and STATUS (PD) at the vector in basil-sim and MDB.
; MDB: SLEEPAT=8 (SLEEP is the instruction already fetched): cycle 28, TOSL 1Eh, STATUS 18h (SLEEP ran as a NOP).
;      SLEEPAT=9 (SLEEP at the stacked PC): 28, 1Eh, 18h.
;      SLEEPAT=7 (the request arrives during SLEEP): 1027, 1Eh, 18h; basil-sim wakes at once (28, PD = 0): not matched.
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
    REPT SLEEPAT
    nop
    ENDM
    sleep
    REPT 20
    nop
    ENDM
    goto chain
END
