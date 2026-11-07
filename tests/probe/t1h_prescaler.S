; Probe (PIC16F183xx): does a write to TMR1H clear the Timer1 prescaler? DS40001839E 27.3 p.286 says a write to TMR1H
; or TMR1L clears it; 27.4 says only TMR1L does (in RD16 mode, which this part's T1CON does not have, Register 27-1).
; Timer1 on Fosc/4 with a 1:8 prescaler runs PRE cycles, then (WRITEH = 1) CLRF TMR1H; TMR1L is then sampled every two
; cycles into 20h-27h, POST cycles after the write. Compare the samples, and the run with WRITEH = 0, between basil-sim
; and MDB. NEWT1 = 1 builds it for a part with T1CLK (PIC16F17146 etc.: T1CLK = Fosc/4, T1CON = 31h is CKPS 1:8, RD16,
; ON), to compare the start of the count after TMR1ON is set (assumption A110).
#include <xc.inc>
PSECT resetVec,class=CODE,reloc=2,delta=2
    goto start
PSECT code,class=CODE,reloc=2,delta=2
start:
    banksel(TMR1L)
    clrf TMR1L&0x7F
    clrf TMR1H&0x7F
    movlw 0x20
    movwf FSR0L
    clrf FSR0H
IF NEWT1
    banksel(T1CLK)
    movlw 1
    movwf T1CLK&0x7F
    banksel(T1CON)
    movlw 0x31
    movwf T1CON&0x7F
    banksel(TMR1L)
ELSE
    movlw 0x31              ; TMR1CS = 00 (Fosc/4), T1CKPS = 11 (1:8), TMR1ON
    movwf T1CON
ENDIF
    REPT PRE
    nop
    ENDM
IF WRITEH
    clrf TMR1H&0x7F
ELSE
    nop
ENDIF
    REPT POST
    nop
    ENDM
    REPT 8
    movf TMR1L-(TMR1L/128)*128,w    ; the offset in the bank: `&` is the concatenation operator inside REPT
    movwi FSR0++
    ENDM
done:
    nop
    goto done
END
