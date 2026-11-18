; Probe (PIC18, Q10): 16-bit Timer0 (Fosc/4, 1:1) enabled by MOVWF T0CON0, then NOPS NOPs, then TMR0L read by
; READ = 0: MOVF TMR0L,W then MOVWF 20h; READ = 1: MOVFF TMR0L,20h; READ = 2: MOVFF TMR0L,20h behind an RCALL.
; PRE > 0 first runs Timer0 8-bit 1:2 for PRE NOPs and stops it (a leftover prescaler count).
; The byte at 20h is the count the read saw; the cycle count at `done` is the same in both engines.
; Run: DFP="$HOME/.mchp_packs/Microchip/PIC18F-Q_DFP/1.31.492/xc8" tools/probe.py OUT tests/probe/t0_read_phase.S 18F27Q10 'READ=2 NOPS=0 PRE=1'
; READ, NOPS and PRE are -D defines (defaults below: READ=2, NOPS=0, PRE=0).
#include <xc.inc>
#ifndef READ
#define READ 2
#endif
#ifndef NOPS
#define NOPS 0
#endif
#ifndef PRE
#define PRE 0
#endif
PSECT resetVec,class=CODE,reloc=2
    goto start
PSECT code,class=CODE,reloc=2
start:
#if PRE > 0
    movlw 0x41
    movwf T0CON1,c
    movlw 0x80
    movwf T0CON0,c
    REPT PRE
    nop
    ENDM
#endif
    clrf T0CON0,c
    movlw 0x40
    movwf T0CON1,c
    clrf TMR0L,c
    clrf TMR0H,c
    movlw 0x90
    movwf T0CON0,c
    REPT NOPS
    nop
    ENDM
#if READ == 0
    movf TMR0L,w,c
    movwf 0x20,c
#elif READ == 1
    movff TMR0L,0x20
#else
    rcall rd
#endif
done:
    nop
    bra done
rd:
    movff TMR0L,0x20
    return
END
