; Probe: the first count of Timer TN (1, 3 or 5) after its ON bit goes 0 -> 1 (assumption A110: one cycle later than the
; [A2] rule for a running timer; measured on Timer1). Timer TN on Fosc/4 with a 1:1 prescaler is started, then after
; PHASE NOPs (0 or 1) TMRnL is sampled every two cycles into 20h-27h. NEWT = 1 builds it for a part with TxCLK
; (CS = 00001 Fosc/4, DS40002343F Table 23-4 p.264; TxCON = 01h: CKPS 1:1, ON); NEWT = 0 for the PIC16F183xx
; (TxCON = 01h: TMRxCS = 00 Fosc/4, TxCKPS 1:1, TMRxON, DS40001839E Register 27-1 p.293).
; Compare the samples between basil-sim and MDB.
#include <xc.inc>
#if TN == 1
TL  equ TMR1L
TH  equ TMR1H
TC  equ T1CON
#if NEWT
TK  equ T1CLK
#endif
#endif
#if TN == 3
TL  equ TMR3L
TH  equ TMR3H
TC  equ T3CON
#if NEWT
TK  equ T3CLK
#endif
#endif
#if TN == 5
TL  equ TMR5L
TH  equ TMR5H
TC  equ T5CON
#if NEWT
TK  equ T5CLK
#endif
#endif
PSECT resetVec,class=CODE,reloc=2,delta=2
    goto start
PSECT code,class=CODE,reloc=2,delta=2
start:
    movlw 0x20
    movwf FSR0L
    clrf FSR0H
#if NEWT
    banksel(TK)
    movlw 1
    movwf TK-(TK/128)*128
#endif
    banksel(TL)
    clrf TL-(TL/128)*128
    clrf TH-(TH/128)*128
    banksel(TC)
    movlw 0x01
    movwf TC-(TC/128)*128
    banksel(TL)
    REPT PHASE
    nop
    ENDM
    REPT 8
    movf TL-(TL/128)*128,w
    movwi FSR0++
    ENDM
done:
    nop
    goto done
END
