; Probe (PIC16F17146 family, NEWT = 1): CCP1 PWM on Timer2 (Fosc/4, 1:1, T2PR = PR) with duty DUTY; if T2 = 0 Timer2 stays off.
; CCP1CON (bit 5 is the output latch, OUT) is read NS times in a row (two cycles apart) into 20h..2Fh after Timer2
; is started with prescaler 1:2^SH, so the bytes show the PWM waveform; ALIGN NOPs before the reads move the sampling by a cycle, DLY more NOPs move the window
; (use DLY to see a later period). Compare the bytes and the cycle count between basil-sim and MDB.
#include <xc.inc>
PIRR equ PIRN&0x7F
CONR equ CCP1CON&0x7F
PSECT resetVec,class=CODE,reloc=2,delta=2
    goto start
PSECT code,class=CODE,reloc=2,delta=2
start:
    banksel(CCPR1L)
    movlw DUTY&0xFF
    movwf CCPR1L&0x7F
    movlw DUTY>>8
    movwf CCPR1H&0x7F
    banksel(CCP1CON)
    movlw 0x8C
    movwf CCP1CON&0x7F
#if NEWT
    banksel(T2CLKCON)
    clrf T2HLT&0x7F
    movlw 1
    movwf T2CLKCON&0x7F
    movlw PR
    movwf T2PR&0x7F
    clrf T2TMR&0x7F
#else
    banksel(PR2)
    movlw PR
    movwf PR2&0x7F
    clrf TMR2&0x7F
#endif
    movlw 0x20
    movwf FSR0L
    clrf FSR0H
    banksel(PIRN)
    clrf PIRR
#if T2
    banksel(T2CON)
#if NEWT
    movlw 0x80|(SH<<4)
#else
    movlw 0x04|SH
#endif
    movwf T2CON&0x7F
#endif
    banksel(CCP1CON)
    REPT ALIGN+DLY
    nop
    ENDM
    REPT NS
    movf CONR,w
    movwi FSR0++
    ENDM
done:
    nop
    goto done
END
