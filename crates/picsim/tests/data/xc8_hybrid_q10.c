/* Locals on XC8's software stack (-mstack=reentrant or hybrid): rec() recurses, so its parameters and locals live in a
   frame at FSR1 and every level of the recursion has its own. done() is where the tests stop. */
#include <xc.h>
#include <stdint.h>

struct pt { int8_t x; int16_t y; };
volatile int16_t out;
volatile uint8_t depth_seen;
volatile uint8_t * volatile gp;

void done(void) { NOP(); }

int16_t sum4(int16_t a, int16_t b, int16_t c, int16_t d) {
    int16_t t = a + b;
    int16_t u = c + d;
    return t + u;
}

uint16_t rec(uint8_t n, uint8_t tag) {
    uint8_t buf[4];
    struct pt p;
    buf[0] = n; buf[1] = tag; buf[2] = 0x5A; buf[3] = n + tag;
    p.x = (int8_t)-n; p.y = 1000 + n;
    if (n == 0) { gp = &buf[1]; depth_seen = tag; done(); return 1; }
    return buf[3] + (uint16_t)p.y + rec(n - 1, tag + 1);
}

/* nest() calls mix(loc[0], leaf(loc[1], 5), loc[2]): XC8 pushes some of mix's arguments before it calls leaf, so while
   leaf runs FSR1 is above nest's frame. mark() is where the tests stop. All three recurse, so they are on the stack. */
void mark(void) { NOP(); }
uint8_t leaf(uint8_t a, uint8_t b) { if (a > 200) return leaf(a - 1, b); mark(); return a + b; }
uint8_t mix(uint8_t a, uint8_t b, uint8_t c) { if (a > 200) return mix(a - 1, b, c); return a ^ b ^ c; }
uint8_t nest(uint8_t n) {
    uint8_t loc[3];
    loc[0] = n; loc[1] = 0x77; loc[2] = n + 1;
    if (n > 200) return nest(n - 1);
    return mix(loc[0], leaf(loc[1], 5), loc[2]);
}

void main(void) {
    int16_t k = 7;
    out = sum4(1, 2, 3, k);
    out = (int16_t)rec(3, 10);
    done();
    out = nest(4);
    while (1);
}
