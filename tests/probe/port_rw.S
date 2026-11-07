; Probe: PORTA / LATA / TRISA / ANSELA. 20h: PORTA with ANSELA at its reset value; 21h: PORTA with ANSELA = 0, TRISA =
; 0xFC, LATA = 3 (RA1:0 outputs, nothing drives the inputs); 22h: PORTA after MOVWF PORTA with 0x0A; 23h: LATA then;
; 24h: LATA after BSF PORTA,5; 25h: TRISA read back after writing 0xFF; 26h: LATA after INCF PORTA,F with LATA = 0x01;
; 27h: PORTA with TRISA = 0x00, ANSELA = 0 and LATA = 0xFF.
#include <xc.inc>
PSECT resetVec,class=CODE,reloc=2,delta=2
    goto start
PSECT code,class=CODE,reloc=2,delta=2
start:
    banksel(PORTA)
    movf PORTA&0x7F,w
    movwf 0x70
    banksel(ANSELA)
    clrf ANSELA&0x7F
    banksel(TRISA)
    movlw 0xFC
    movwf TRISA&0x7F
    banksel(LATA)
    movlw 3
    movwf LATA&0x7F
    banksel(PORTA)
    movf PORTA&0x7F,w
    movwf 0x71
    movlw 0x0A
    movwf PORTA&0x7F
    movf PORTA&0x7F,w
    movwf 0x72
    banksel(LATA)
    movf LATA&0x7F,w
    movwf 0x73
    banksel(PORTA)
    bsf PORTA&0x7F,5
    banksel(LATA)
    movf LATA&0x7F,w
    movwf 0x74
    banksel(TRISA)
    movlw 0xFF
    movwf TRISA&0x7F
    movf TRISA&0x7F,w
    movwf 0x75
    banksel(LATA)
    movlw 1
    movwf LATA&0x7F
    banksel(PORTA)
    incf PORTA&0x7F,f
    banksel(LATA)
    movf LATA&0x7F,w
    movwf 0x76
    banksel(TRISA)
    clrf TRISA&0x7F
    banksel(LATA)
    movlw 0xFF
    movwf LATA&0x7F
    banksel(PORTA)
    movf PORTA&0x7F,w
    movwf 0x77
    banksel(0x20)
    movf 0x70,w
    movwf 0x20
    movf 0x71,w
    movwf 0x21
    movf 0x72,w
    movwf 0x22
    movf 0x73,w
    movwf 0x23
    movf 0x74,w
    movwf 0x24
    movf 0x75,w
    movwf 0x25
    movf 0x76,w
    movwf 0x26
    movf 0x77,w
    movwf 0x27
done:
    nop
    goto done
