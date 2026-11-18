; Probe (PIC18, Q10): how long one EUSART1 asynchronous character takes, in instruction cycles, for a given BRG mode.
; BRG16 / BRGH select the mode (TX9 = 1: nine-bit characters) (DS40002043F Table 29-1 p.511), N = SP1BRGH:SP1BRGL. NBYTES characters are written
; one by one to TX1REG; after each, a counting loop (7 cycles per pass, 6 on the last) waits for TRMT (TX1STA.1) and
; the passes are added up in the 16-bit counter at 20h (low) and 21h. The count x 7 is about the cycles spent sending,
; the same in both engines (it does not use the stopwatch).
; NBYTES is a -D define too, e.g. 'BRG16=1 BRGH=1 N=15 NBYTES=2'.
; Run: DFP="$HOME/.mchp_packs/Microchip/PIC18F-Q_DFP/1.31.492/xc8" MAXCYC=2000000 tools/probe.py OUT tests/probe/eusart_char.S 18F27Q10 'BRG16=1 BRGH=1 N=15'
#include <xc.inc>
#ifndef BRG16
#define BRG16 1
#endif
#ifndef BRGH
#define BRGH 1
#endif
#ifndef N
#define N 15
#endif
#ifndef TX9
#define TX9 0
#endif
#ifndef NBYTES
#define NBYTES 1
#endif
PSECT resetVec,class=CODE,reloc=2
start:
    banksel(ANSELC)
    bcf ANSELC,6,b
    banksel(RC6PPS)
    movlw 0x09
    movwf RC6PPS,b
    banksel(TRISC)
    bcf TRISC,6,b
    clrf 0x20,c
    clrf 0x21,c
    movlw (N & 0xFF)
    movwf SP1BRGL,c
    movlw ((N >> 8) & 0xFF)
    movwf SP1BRGH,c
#if BRG16
    movlw 0x08
    movwf BAUD1CON,c
#else
    clrf BAUD1CON,c
#endif
#if BRGH
    movlw 0x24
#else
    movlw 0x20
#endif
#if TX9
    iorlw 0x40
#endif
    movwf TX1STA,c
    movlw 0x80
    movwf RC1STA,c
    ; NBYTES characters
    movlw NBYTES
    movwf 0x22,c
sendloop:
    movlw 0x55
    movwf TX1REG,c
    nop
    nop
wait:
    movlw 1
    addwf 0x20,f,c
    movlw 0
    addwfc 0x21,f,c
    btfss TX1STA,1,c
    bra wait
    decfsz 0x22,f,c
    bra sendloop
done:
    bra done
