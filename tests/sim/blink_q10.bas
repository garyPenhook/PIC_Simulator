' Blinks the "heartbeat" LED (RB0, active high) at 1 Hz and lights the "error" LED (RB1, active low) while
' RB0 is off, so the two alternate. Pair with tests/sim/board_example.stim.toml (its OLED is left unused).
' Run:  basil tests/sim/blink_q10.bas && basil-sim tests/sim/blink_q10.hex --device 18F27Q10 --stim tests/sim/blink.stim.toml
Program Blink
Device = 18F27Q10
Clock = 64
Config FEXTOSC = OFF, RSTOSC = HFINTOSC_64MHZ, WDTE = OFF, LVP = ON
Dim led As PORTB.0
Dim err As PORTB.1
TRISB = 0
While True
    led = 1
    err = 1
    DelayMs(500)
    led = 0
    err = 0
    DelayMs(500)
Wend
