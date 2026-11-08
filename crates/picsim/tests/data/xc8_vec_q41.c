// Vectored interrupts (MVECEN = 1) on the PIC18F16Q41: Timer0 low priority, Timer2 high priority.
#include <xc.h>
#pragma config MVECEN = ON
#pragma config WDTE = OFF
#pragma config LVP = ON

volatile unsigned char t0n, t2n, order;

void __interrupt(irq(TMR0), base(8), low_priority) t0isr(void) {
    PIR3bits.TMR0IF = 0;
    t0n++;
    if (!order) order = 1;
}

void __interrupt(irq(TMR2), base(8), high_priority) t2isr(void) {
    PIR3bits.TMR2IF = 0;
    t2n++;
    if (!order) order = 2;
}

void main(void) {
    T0CON0 = 0x80;          // enabled, 8-bit
    T0CON1 = 0x40;          // Fosc/4, no prescale
    T2CON = 0x80;           // on
    T2PR = 100;
    T2CLK = 1;
    PIE3bits.TMR0IE = 1;
    PIE3bits.TMR2IE = 1;
    IPR3bits.TMR0IP = 0;
    IPR3bits.TMR2IP = 1;
    INTCON0bits.IPEN = 1;
    INTCON0bits.GIEH = 1;
    INTCON0bits.GIEL = 1;
    while (1) {
    }
}
