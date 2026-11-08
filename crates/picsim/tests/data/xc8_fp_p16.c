#include <xc.h>
#pragma config WDTE = OFF, LVP = ON
volatile unsigned char sink;
void a(void) { sink = 1; }
void b(void) { sink = 2; }
void (*fn)(void);
void (*fn2)(void);
void (*tbl[2])(void);
void main(void)
{
    fn = a; fn2 = b; tbl[0] = b; tbl[1] = a;
    fn();
    fn2();
    tbl[0]();
    while (1) { }
}
