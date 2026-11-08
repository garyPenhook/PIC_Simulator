#include <xc.h>
#pragma config WDTE = OFF, LVP = ON
volatile unsigned char sink;
void main(void)
{
    ANSELA = 0;
    TRISA = 0xFE;           /* RA0 is an output */
    RA0PPS = 0x09;          /* CCP1 */
    T2CLKCON = 1;           /* Fosc/4 */
    T2PR = 0x0F;            /* 16 clocks a period */
    T2CON = 0x80;           /* on, 1:1 */
    CCPR1L = 20;            /* duty */
    CCPR1H = 0;
    CCP1CON = 0x8C;         /* PWM */
    while (1) { sink = PORTA; }
}
