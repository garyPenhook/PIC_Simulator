; Probe: software makes an interrupt pending while GIE is already set.
; VARIANT=1: bsf PIR0,TMR0IF (TMR0IE, PEIE, GIE already set)
; VARIANT=2: bsf PIE0,TMR0IE (TMR0IF already set by hand, PEIE, GIE set)
; CHAIN=1: followed by nops, CHAIN=2: by 2-cycle bra $+1
; Read the cycle of the vector entry and the stacked return address (TOSL). MDB, all four builds: vector at cycle 13,
; TOSL = 0Fh (the first chain instruction): taken right after the BSF, as a request already pending ([A104]).
#include <xc.inc>
PSECT resetVec,class=CODE,reloc=2,delta=2
    goto start
PSECT intVec,class=CODE,reloc=2,delta=2
isr:
    goto isr
PSECT code,class=CODE,reloc=2,delta=2
start:
    movlb 0
    bsf INTCON,6
#if VARIANT==1
    banksel(PIE0)
    bsf PIE0&0x7F,5
    movlb 0
    bsf INTCON,7
    banksel(PIR0)
    nop
    bsf PIR0&0x7F,5
#else
    banksel(PIR0)
    bsf PIR0&0x7F,5
    movlb 0
    bsf INTCON,7
    banksel(PIE0)
    nop
    bsf PIE0&0x7F,5
#endif
chain:
#if CHAIN==1
    REPT 8
    nop
    ENDM
#else
    REPT 4
    bra $+1
    ENDM
#endif
    goto chain
END
