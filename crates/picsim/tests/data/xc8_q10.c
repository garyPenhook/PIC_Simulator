#include <xc.h>
#pragma config WDTE = OFF, LVP = ON
volatile unsigned int counter;
static unsigned char add(unsigned char a, unsigned char b)
{
    return a + b;
}
void main(void)
{
    unsigned char x = 1;
    TRISB = 0;
    for (int i = 0; i < 5; i++) {
        x = add(x, 2);
        counter += x;
        LATB = x;
    }
    while (1) {
        counter++;
    }
}
