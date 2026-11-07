' basil-sim only (needs tests/sim/adc_q41.stim.toml; PIC16F17146 ADCC): the ADC library reads RC0 (channel 16) and the FVR buffer
' channel after the stimulus file has driven them; the 12-bit result is right justified. Channel 17 has no stimulus and reads 0.
Program Adp
Device = 16F17146
Clock = 32
Config FEXTOSC = OFF, RSTOSC = HFINTOSC_32MHz, WDTE = OFF, LVP = ON
Include "adc"
Dim a0 As Word
Dim a1 As Word
Dim a2 As Word
Adc.AdcInit()
a0 = ADin(16)
a1 = ADin(17)
a2 = ADin(16)
