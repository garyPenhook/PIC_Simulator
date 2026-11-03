' Hardware probe: Timer0 8-bit, period 100 clocks, prescaler 1:4. Timer1 (FOSC/4, 1:1) is the stopwatch started at
' the same instruction that enables Timer0; ELAPSED is Timer1 when TMR0IF is seen. Loop variant: movf.
' Simulator flavour: result in ELAPSED, compare with the chip.
Program Hp
Device = 18F27Q10
Clock = 64
Config FEXTOSC = OFF, RSTOSC = HFINTOSC_64MHZ, WDTE = OFF, LVP = ON

Dim tf As PIR0.5
Dim z As Byte
Dim y As Byte
Dim elapsed As Word
Dim r As Byte
T1CLK = 1
T1CON = 3
TMR0H = 99
T0CON1 = 66
T0CON0 = 128
TMR1H = 0
TMR1L = 0
While tf = 0
    If T0CON0 = 0 Then
        z = 5
    End If
Wend
elapsed = TMR1

