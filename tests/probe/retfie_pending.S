; Probe: a Timer0 interrupt that is still pending when RETFIE executes (the handler leaves TMR0IF set the first
; time). What runs at the return address X, what does the stack hold, and when is the vector reached?
; Build: VARIANT=1 incf (1 cycle), 2 goto (2 cycles), 3 nop, 4 btfss that skips (2 cycles)
#include <xc.inc>
cnt equ 0x20
n   equ 0x21
PSECT resetVec,class=CODE,reloc=2,delta=2
    goto start
PSECT intVec,class=CODE,reloc=2,delta=2
isr:                       ; 0x0004
    movlb 0
    incf n,f
    movlw 2
    xorwf n,w
    btfsc STATUS,2         ; second entry: clear the flag
    bcf PIR0&0x7F,5
    retfie
PSECT code,class=CODE,reloc=2,delta=2
start:
    movlb 0
    clrf cnt
    clrf n
    banksel(PIE0)
    bsf PIE0&0x7F,5        ; TMR0IE
    banksel(PIR0)
    bsf PIR0&0x7F,5        ; TMR0IF, set by hand
    movlb 0
    bsf INTCON,6           ; PEIE
    bsf INTCON,7           ; GIE: the interrupt is taken here
X:
#if VARIANT==1
    incf cnt,f
#elif VARIANT==2
    goto X2
X2:
    incf cnt,f
#elif VARIANT==3
    nop
    incf cnt,f
#else
    bsf STATUS,2
    btfss STATUS,2
    incf cnt,f
    incf cnt,f
#endif
    nop
    nop
done:
    goto done
END
