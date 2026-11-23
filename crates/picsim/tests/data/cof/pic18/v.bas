Device = 18F27Q10
Declare Xtal = 64
Declare Create_Coff = On
Config_Start
  FEXTOSC = OFF
  RSTOSC = HFINTOSC_64MHZ
  WDTE = OFF
  LVP = ON
Config_End
Dim vb As Byte
Dim vw As Word
Dim vd As Dword
Dim vsb As SByte
Dim vsw As SWord
Dim vsd As SDword
Dim vf As Float
Dim vs As String * 10
Dim arr[5] As Byte
Dim warr[3] As Word
Dim flag As Bit

Proc Calc(a As Word, b As SByte), Word
  Dim loc As Byte
  loc = a
  Result = loc + b
EndProc

Proc Twice(x As Byte), Byte
  Result = x * 2
EndProc

vb = 1
vw = Calc(5, 2)
vb = Twice(vb)
arr[1] = 7
vs = "hi"
flag = 1
vf = 1.5
vsb = -3
vsw = -300
vsd = -70000
vd = 100000
warr[2] = 513
Stop
