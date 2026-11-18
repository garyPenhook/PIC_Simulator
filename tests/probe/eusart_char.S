; Probe (PIC18, Q10): how long one EUSART1 asynchronous character takes, in instruction cycles, for a given BRG mode.
; BRG16 / BRGH select the mode (TX9 = 1: nine-bit characters; K22 = 1: PIC18F25K22 register names, no PPS) (DS40002043F Table 29-1 p.511), N = SP1BRGH:SP1BRGL. NBYTES characters are written
; one by one to TX1REG; after each, a counting loop (7 cycles per pass, 6 on the last) waits for TRMT (TXSTA.1) and
; the passes are added up in the 16-bit counter at 20h (low) and 21h. The count x 7 is about the cycles spent sending,
; the same in both engines (it does not use the stopwatch).
; NBYTES is a -D define too, e.g. 'BRG16=1 BRGH=1 N=15 NBYTES=2'.
; Run (K22: DFP from PIC18F-K_DFP, part 18F25K22, K22=1 in the variant): DFP="$HOME/.mchp_packs/Microchip/PIC18F-Q_DFP/1.31.492/xc8" MAXCYC=2000000 tools/probe.py OUT tests/probe/eusart_char.S 18F27Q10 'BRG16=1 BRGH=1 N=15'
#include <xc.inc>
#ifndef K22
#define K22 0
#endif
#if K22
#define UBRGL SPBRG1
#define UBRGH SPBRGH1
#define UBAUD BAUDCON1
#define UTXSTA TXSTA1
#define URCSTA RCSTA1
#define UTXREG TXREG1
#else
#define UBRGL SP1BRGL
#define UBRGH SP1BRGH
#define UBAUD BAUD1CON
#define UTXSTA TX1STA
#define URCSTA RC1STA
#define UTXREG TX1REG
#endif
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
#if !K22
    banksel(RC6PPS)
    movlw 0x09
    movwf RC6PPS,b
#endif
    banksel(TRISC)
    bcf TRISC,6,b
    clrf 0x20,c
    clrf 0x21,c
    movlw (N & 0xFF)
    movwf UBRGL,c
    movlw ((N >> 8) & 0xFF)
    movwf UBRGH,c
#if BRG16
    movlw 0x08
    movwf UBAUD,c
#else
    clrf UBAUD,c
#endif
#if BRGH
    movlw 0x24
#else
    movlw 0x20
#endif
#if TX9
    iorlw 0x40
#endif
    movwf UTXSTA,c
    movlw 0x80
    movwf URCSTA,c
    ; NBYTES characters
    movlw NBYTES
    movwf 0x22,c
sendloop:
    movlw 0x55
    movwf UTXREG,c
    nop
    nop
wait:
    movlw 1
    addwf 0x20,f,c
    movlw 0
    addwfc 0x21,f,c
    btfss UTXSTA,1,c
    bra wait
    decfsz 0x22,f,c
    bra sendloop
done:
    bra done
