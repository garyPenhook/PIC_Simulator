' basil-sim only: the library Spi host on SPI1 of the PIC18F16Q41 against a stimulus client ([[spi]]).
' Pins: SCK RC0 (16), SDO RC1 (17), SDI RC2 (18).
Program Sq1
Device = 18F16Q41
Clock = 64
Config FEXTOSC = OFF, RSTOSC = HFINTOSC_64MHZ, WDTE = OFF, LVP = ON
Include "spi"
Dim sq_init As Byte
Dim sq_r0 As Byte
Dim sq_r1 As Byte
Dim sq_r2 As Byte
Dim sq_st As Byte
sq_init = Spi.SpiInit(1000000, 0, 16, 17, 18)
sq_r0 = Spi.SpiTransfer(159)
sq_r1 = Spi.SpiTransfer(0)
sq_r2 = Spi.SpiRead()
sq_st = Spi.SpiStatus()
