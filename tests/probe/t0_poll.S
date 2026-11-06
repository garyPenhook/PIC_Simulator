; Probe: Timer0 (Fosc/4, 8-bit, 1:1, TMR0H = PERIOD) started, then TMR0IF polled; `done` is reached when the flag is seen.
; The cycle count at `done` in basil-sim and MDB shows whether the flag is set at the same cycle (no interrupts involved).
#include <xc.inc>
PSECT resetVec,class=CODE,reloc=2,delta=2
    goto start
PSECT code,class=CODE,reloc=2,delta=2
start:
    movlb 0
    banksel(T0CON1)
    movlw 0x40
    movwf T0CON1&0x7F
    banksel(TMR0H)
    movlw PERIOD
    movwf TMR0H&0x7F
    banksel(T0CON0)
    movlw 0x80
    movwf T0CON0&0x7F
    banksel(PIR0)
    REPT PRE
    nop
    ENDM
poll:
    btfss PIR0&0x7F,5
    goto poll
done:
    nop
    goto done
END
