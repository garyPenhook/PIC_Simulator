Device = 16F18313
Declare Xtal = 32
Declare Create_Coff = On
Dim total As Word
Dim i As Byte

Proc AddIt(n As Byte)
  total = total + n
EndProc

total = 0
For i = 1 To 5
  AddIt(i)
Next
End
