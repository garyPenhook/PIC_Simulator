/* The second file of the xc8_multi fixture (see xc8_multi_main.c). */
#include <xc.h>
#include <stdint.h>

volatile uint16_t shared = 0x1234;

static volatile uint8_t count = 0x22;

static void helper(void) { count += 2; }

void stop(void) { NOP(); }

uint8_t bump(uint8_t k) {
    uint8_t loc = k + count;
    helper();
    stop();
    return loc;
}
