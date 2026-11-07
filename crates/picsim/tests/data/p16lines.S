#include <xc.inc>
PSECT code,class=CODE,abs,delta=2
    ORG 0x0000
__reset:
#line 1 "p16lines.bas"
    nop
#line 2 "p16lines.bas"
    movlw 5
#line 2 "p16lines.bas"
    movwf 0x20
#line 3 "p16lines.bas"
    call sub
#line 4 "p16lines.bas"
    movlw 7
#line 5 "p16lines.bas"
__halt:
    goto __halt
sub:
#line 10 "p16lines.bas"
    movlw 1
#line 10 "p16lines.bas"
    movwf 0x21
#line 11 "p16lines.bas"
    return
    END __reset
