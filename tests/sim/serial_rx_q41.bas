' basil-sim only (needs tests/sim/serial_rx.stim.toml; PIC18F16Q41 UART1): the Serial library reads three bytes the stimulus file sends
' to the EUSART1 receiver, echoes each, and then reads the fourth, which has a bad Stop bit (FERR).
Program Srx
Device = 18F16Q41
Clock = 64
Config FEXTOSC = OFF, RSTOSC = HFINTOSC_64MHZ, WDTE = OFF, LVP = ON
Include "serial"
Dim b0 As Byte
Dim b1 As Byte
Dim b2 As Byte
Dim b3 As Byte
Dim ferr As Byte
Serial.SerialInit(115200)
b0 = Serial.SerialRead()
Serial.SerialWriteByte(b0)
b1 = Serial.SerialRead()
Serial.SerialWriteByte(b1)
b2 = Serial.SerialRead()
Serial.SerialWriteByte(b2)
While Not Serial.SerialAvailable()
Wend
ferr = U1ERRIR And 8
b3 = Serial.SerialRead()
Serial.SerialFlush()
