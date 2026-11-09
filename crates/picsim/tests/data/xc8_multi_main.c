/* Two C files (with xc8_multi_util.c): `shared` is defined there and declared extern here, each file has its own static
   `count` and its own static `helper`, and main calls bump() in the other file. stop() is where the tests stop. */
#include <xc.h>
#include <stdint.h>

extern volatile uint16_t shared;
uint8_t bump(uint8_t k);
void stop(void);

static volatile uint8_t count = 0x11;

static void helper(void) { count++; }

void main(void) {
    helper();
    shared += bump(3);
    stop();
    while (1);
}
