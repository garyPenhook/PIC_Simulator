; Probe: Timer1 (Fosc/4, 1:1) started at FFFFh - PERIOD, then TMR1IF polled; `done` is reached when the flag is seen.
; CLR = 1, 2, 3 put a BCF of TMR1IF, a CLRF PIR1 or a MOVWF PIR1 of 0 after the PRE NOPs (CLR = 0: none), to see whether
; a clear in the cycle the overflow sets the flag wins. NEWT = 1 builds it for a part with T1CLK (PIC16F17146: T1CLK =
; 01h Fosc/4, DS40002343F Table 23-4 p.264; T1CON = 01h ON; TMR1IF is PIR1 bit 6); NEWT = 0 for the PIC16F183xx
; (T1CON = 01h: TMR1CS = 00 Fosc/4, 1:1, TMR1ON, DS40001839E Register 27-1 p.293; TMR1IF is PIR1 bit 0).
; Compare the cycle count at `done` between basil-sim and MDB.
#include <xc.inc>
#if NEWT
FB  equ 6
#else
FB  equ 0
#endif
PSECT resetVec,class=CODE,reloc=2,delta=2
    goto start
PSECT code,class=CODE,reloc=2,delta=2
start:
#if NEWT
    banksel(T1CLK)
    movlw 1
    movwf T1CLK&0x7F
#endif
    banksel(TMR1L)
    movlw 0xFF
    movwf TMR1H&0x7F
    movlw 0xFF-PERIOD
    movwf TMR1L&0x7F
    banksel(PIR1)
    clrf PIR1&0x7F
    banksel(T1CON)
    movlw 0x01
    movwf T1CON&0x7F
    banksel(PIR1)
    movlw 0
    REPT PRE
    nop
    ENDM
#if CLR == 1
    bcf PIR1&0x7F,FB
#endif
#if CLR == 2
    clrf PIR1&0x7F
#endif
#if CLR == 3
    movwf PIR1&0x7F
#endif
poll:
    btfss PIR1&0x7F,FB
    goto poll
done:
    nop
    goto done
END
