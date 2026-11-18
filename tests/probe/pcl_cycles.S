; DS40002043F instruction summary note 3 (p.630): writing PCL adds a pipeline flush cycle.
; MDB is compared explicitly; no attempt is made to copy an MDB timing defect.
#include <xc.inc>
PSECT resetVec,class=CODE,reloc=2
    movlw 0
    addwf PCL,f,c
    nop
done:
    nop
