' basil-sim only: the library Spi host on MSSP1 of the PIC18F27Q10 against a stimulus client ([[spi]]).
' Pins: SCK RC3 (19), SDO RC5 (21), SDI RC4 (20).
Program Sq10
Device = 18F27Q10
Clock = 64
Config FEXTOSC = OFF, RSTOSC = HFINTOSC_64MHZ, WDTE = OFF, LVP = ON
Include "spi"
Dim sq_init As Byte
Dim sq_r0 As Byte
Dim sq_r1 As Byte
Dim sq_r2 As Byte
Dim sq_st As Byte
sq_init = Spi.SpiInit(1000000, 0, 19, 21, 20)
sq_r0 = Spi.SpiTransfer(159)
sq_r1 = Spi.SpiTransfer(0)
sq_r2 = Spi.SpiRead()
sq_st = Spi.SpiStatus()
