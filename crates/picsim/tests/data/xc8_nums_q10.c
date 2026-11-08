#include <xc.h>
#pragma config WDTE = OFF, LVP = ON
volatile float f = 3.5f;
volatile double d = -2.25;
volatile long double ld = 1.5;
volatile long long ll = -5000000000LL;
volatile unsigned long long ull = 0x1122334455667788ULL;
volatile _Bool flag = 1;
void main(void) { while (1) { } }
