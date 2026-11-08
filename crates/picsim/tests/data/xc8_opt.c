#include <xc.h>
#pragma config WDTE = OFF, LVP = ON
volatile unsigned char sink;
struct pt { unsigned char x; int y; };
struct pt origin = {7, -300};
unsigned char tab[4] = {1,2,3,4};
static unsigned char sq(unsigned char x) { unsigned char y = x * x; return y + 1; }
static unsigned int acc(const unsigned char *p, unsigned char n)
{
    unsigned int s = 0; unsigned char j;
    for (j = 0; j < n; j++) s += p[j];
    return s;
}
unsigned int total;
unsigned char d, e;
void main(void)
{
    unsigned char c = 3;
    struct pt local;
    local.x = sq(c);
    local.y = origin.y + local.x;
    d = local.x;
    e = d + c;
    total = acc(tab, 4);
    sink = e;
    sink = (unsigned char)local.y;
    while (1) { sink = d; }
}
