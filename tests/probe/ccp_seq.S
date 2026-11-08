; Probe (PIC16F17146 family, NEWT = 1; PIC16F183xx, NEWT = 0): CCP1 in compare mode MODE on Timer1 (Fosc/4, 1:1), CCPR1 = CMP.
; After CCP1IF (PIR bit PB of PIRN) is first seen, TMR1L is read eight times in a row (two cycles apart) into 20h..27h, so the
; bytes show where Timer1 is cleared relative to the flag. Compare the cycle count and the bytes between basil-sim and MDB.
#include <xc.inc>
TL  equ TMR1L&0x7F
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
    clrf TMR1H&0x7F
    clrf TMR1L&0x7F
    banksel(CCPR1L)
    movlw CMP&0xFF
    movwf CCPR1L&0x7F
    movlw CMP>>8
    movwf CCPR1H&0x7F
    banksel(PIRN)
    clrf PIRN&0x7F
    banksel(CCP1CON)
    movlw 0x80|MODE
    movwf CCP1CON&0x7F
    banksel(T1CON)
    movlw 0x01
    movwf T1CON&0x7F
    movlw 0x20
    movwf FSR0L
    clrf FSR0H
    banksel(PIRN)
poll:
    btfss PIRN&0x7F,PB
    goto poll
    banksel(TMR1L)
    REPT 8
    movf TL,w
    movwi FSR0++
    ENDM
done:
    nop
    goto done
END
