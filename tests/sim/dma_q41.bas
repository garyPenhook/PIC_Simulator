' Native Q41 DMA integration: the transfer must actually copy and complete.
Program Dnative
Device = 18F16Q41
Clock = 64
Config FEXTOSC = OFF, RSTOSC = HFINTOSC_64MHZ, WDTE = OFF, LVP = ON
Include "dma"
Dim source(4) As Byte
Dim dest(4) As Byte
Dim started As Byte
Dim done As Byte
Dim ok As Byte
source(0) = 18
source(1) = 52
source(2) = 86
source(3) = 120
started = Dma.DmaCopy(1, @source, @dest, 4)
While Dma.DmaDone(1) = 0
Wend
done = Dma.DmaDone(1)
If dest(0) = 18 And dest(1) = 52 And dest(2) = 86 And dest(3) = 120 Then
    ok = 1
End If
