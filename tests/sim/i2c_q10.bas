' basil-sim only: the library I2c host on MSSP1 of the PIC18F27Q10 against a stimulus client ([[i2c]]).
' Reads two bytes of register 0x10 from the client at 0x50, then probes an address nobody owns.
' Pins: SCL RC3 (19), SDA RC4 (20).
Program Iq10
Device = 18F27Q10
Clock = 64
Config FEXTOSC = OFF, RSTOSC = HFINTOSC_64MHZ, WDTE = OFF, LVP = ON
Include "i2c"
Dim iq_init As Byte
Dim iq_s0 As Byte
Dim iq_s1 As Byte
Dim iq_s2 As Byte
Dim iq_s3 As Byte
Dim iq_r0 As Byte
Dim iq_r1 As Byte
Dim iq_nak As Byte
iq_init = I2c.I2cInit(100000, 19, 20)
iq_s0 = I2c.I2cStart()
iq_s1 = I2c.I2cSend(160)
iq_s1 = iq_s1 + I2c.I2cSend(16)
iq_s2 = I2c.I2cRestart()
iq_s2 = iq_s2 + I2c.I2cSend(161)
iq_r0 = I2c.I2cReceive(True)
iq_r1 = I2c.I2cReceive(False)
iq_s3 = I2c.I2cStop()
iq_s0 = I2c.I2cStart()
iq_nak = I2c.I2cSend(180)
iq_s3 = I2c.I2cStop()
