' basil-sim only (MPLAB X's UART model never drains the Q41 transmit buffer, so this cannot run in MDB): two bytes
' go out of UART1 of the PIC18F16Q41 back to back and SerialFlush waits for the shift register (TXMTIF).
Program Stx
Device = 18F16Q41
Clock = 64
Config FEXTOSC = OFF, RSTOSC = HFINTOSC_64MHZ, WDTE = OFF, LVP = ON
Include "serial"
Serial.SerialInit(115200)
Serial.SerialWriteByte(65)
Serial.SerialWriteByte(66)
Serial.SerialFlush()
