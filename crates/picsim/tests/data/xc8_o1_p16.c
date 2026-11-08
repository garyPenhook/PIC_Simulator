#include <xc.h>
#pragma config WDTE = OFF, LVP = ON
volatile unsigned char sink;
static unsigned char sq(unsigned char x) { unsigned char y = x * x; return y + 1; }
static unsigned int acc(const unsigned char *p, unsigned char n)
{
    unsigned int s = 0; unsigned char j;
    for (j = 0; j < n; j++) s += p[j];
    return s;
}
unsigned char tab[4] = {1,2,3,4};
void main(void)
{
    unsigned char c = 3, d, e;
    unsigned int total;
    d = sq(c);
    e = d + c;
    total = acc(tab, 4);
    sink = e;
    sink = (unsigned char)total;
    while (1) { sink = d; }
}
