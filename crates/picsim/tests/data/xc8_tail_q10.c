/* A tail call into a routine on XC8's software stack (-O3 or -Os, -mstack=reentrant): inner() recurses, so its parameters
   and locals are in a frame at FSR1. hop() pushes inner's two arguments and, with nothing left to do after the call, ends
   in GOTO _inner (PIC16: LJMP) instead of CALL; RETURN. inner's frame is then above FSR1 as it was when hop was called.
   mark() is where the tests stop. */
#include <xc.h>
#include <stdint.h>

volatile uint8_t g;

void mark(void) { NOP(); }

void inner(uint8_t n, uint8_t m) {
    volatile uint8_t l[2];
    l[0] = n; l[1] = m;
    if (n > 200) inner(n - 1, m);
    mark();
    g = l[0] + l[1];
}

void hop(uint8_t a) { inner(a, 0xA5); }

void main(void) {
    hop(3);
    hop(4);
    while (1);
}
