; Probe (PIC16F17146 family, NEWT = 1; PIC16F183xx, NEWT = 0): CCP1 in compare mode on Timer1 (Fosc/4, 1:1) started from 0000h.
; CCPR1 = CMP, CCP1CON = 80h | MODE. PORTA bit 0 is the CCP1 output (RA0PPS = PPSCODE, TRISA0 = 0). CCP1IF (PIR bit PB of PIR PIRN)
; is polled; at `done` the bytes at 20h.. are PORTA, TMR1L, TMR1H, CCP1CON. Compare the cycle count and the bytes between
; basil-sim and MDB. PRE NOPs go between the flag and the reads. TWICE = 1 clears the flag and waits for it a second time (the
; difference of the cycle counts is the period).
#include <xc.inc>
PSECT resetVec,class=CODE,reloc=2,delta=2
    goto start
PSECT code,class=CODE,reloc=2,delta=2
start:
    banksel(ANSELA)
    clrf ANSELA&0x7F
    banksel(RA0PPS)
    movlw PPSCODE
    movwf RA0PPS&0x7F
    banksel(TRISA)
    movlw 0xFE
    movwf TRISA&0x7F
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
    banksel(PIRN)
poll:
    btfss PIRN&0x7F,PB
    goto poll
#if TWICE
    bcf PIRN&0x7F,PB
poll2:
    btfss PIRN&0x7F,PB
    goto poll2
#endif
stop:
    REPT PRE
    nop
    ENDM
    banksel(PORTA)
    movf PORTA&0x7F,w
    banksel(0x20)
    movwf 0x20
    banksel(TMR1L)
    movf TMR1L&0x7F,w
    banksel(0x20)
    movwf 0x21
    banksel(TMR1H)
    movf TMR1H&0x7F,w
    banksel(0x20)
    movwf 0x22
    banksel(CCP1CON)
    movf CCP1CON&0x7F,w
    banksel(0x20)
    movwf 0x23
done:
    nop
    goto done
END
