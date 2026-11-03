Program rtcc_clock

' Clock and calendar on an external MCP7940N I2C RTCC (the PIC18F16Q41 has no RTCC peripheral).
' The date and time are printed over the serial port once a second and are set from a terminal.
'
' Wiring (all pins are on the 20-pin part):
'   RC0 (pin 16)  -> MCP7940N SCL   with a pull-up to VDD (4.7k)
'   RC1 (pin 17)  -> MCP7940N SDA   with a pull-up to VDD (4.7k)
'   RC4           -> UART1 TX   (to the USB-serial adapter RX)
'   RC5           -> UART1 RX   (from the USB-serial adapter TX)
'   MCP7940N: X1/X2 = 32.768 kHz crystal, VBAT = backup cell (or ground it), VCC = VDD, VSS = GND
' RA0/RA1 are left free for the programmer.
'
' Terminal (9600 8N1, end each line with Enter):
'   S yymmddhhmmss   set date and time    e.g. S261031143000
'   T hhmmss         set the time only    e.g. T143000
'   D yymmdd         set the date only    e.g. D261031
'   ?                help
' Spaces after the command letter are optional. The weekday is worked out from the date.

Device = 18F27q10
Clock = 64

' I2C: the PIC18F16Q41's I2C1 peripheral generates the bus timing (exactly 100 kHz from FOSC/4; an interrupt cannot stretch a bit, and a slow
' client only stretches the clock). The calls still wait for each transfer. Delete the next line to go back to the bit-banged I2C library,
' which needs no peripheral (and Clock >= 24 MHz for its variable DelayUS). Both use RC0 = SCL and RC1 = SDA with the same pull-ups.
#option I2c = Hardware

' PIC18F16Q41 configuration
Config FEXTOSC = OFF, RSTOSC = HFINTOSC_64MHZ
' FEXTOSC = OFF: Oscillator not enabled
' RSTOSC = HFINTOSC_64MHZ: starts at 64 MHz, so Clock matches the hardware with no run-time switch.
' The bit-banged I2c library (without the #option above) needs Clock >= 24 MHz for its variable DelayUS; the I2C1 peripheral does not.
' 64 MHz needs VDD >= 2.7 V (DS40002214G table 48.4.1, OS5): run the board from 3.3 V or 5 V.

Include "gpio"
Include "serial"
Include "i2c"
Include "rtcc"

Const SclPin = 16
Const SdaPin = 17
Const I2cHz = 100000
Const Baud = 9600
Const UseBattery = 1                   ' 1: enable the VBAT input (a cell must be wired to VBAT)
Const LineMax = 14

' Time used when the chip has never been set (year 2000 + 26, 31 October, Saturday = 6, Monday = 1)
Const DefYear = 26
Const DefMonth = 10
Const DefDay = 31
Const DefWeekday = 6

' Month offsets for the weekday formula (Sakamoto), January first.
Const MonthOffset(12) As Byte = (0, 3, 2, 5, 0, 3, 5, 1, 4, 6, 2, 4)

Dim cmd(LineMax) As Byte               ' the line being typed
Dim cmdLen As Byte
Dim cmdBad As Boolean                  ' the line was longer than the buffer
Dim fy As Byte                         ' fields parsed from a command
Dim fm As Byte
Dim fd As Byte
Dim fh As Byte
Dim fmi As Byte
Dim fs As Byte

' ---------------------------------------------------------------------------------------------
' Printing
' ---------------------------------------------------------------------------------------------

' Two decimal digits with a leading zero.
Sub Print2(ByVal v As Byte)
    SerialWriteByte(48 + v / 10)
    SerialWriteByte(48 + v Mod 10)
End Sub

Sub PrintDayName(ByVal d As Byte)
    If d = 1 Then
        SerialWrite("Mon")
    ElseIf d = 2 Then
        SerialWrite("Tue")
    ElseIf d = 3 Then
        SerialWrite("Wed")
    ElseIf d = 4 Then
        SerialWrite("Thu")
    ElseIf d = 5 Then
        SerialWrite("Fri")
    ElseIf d = 6 Then
        SerialWrite("Sat")
    ElseIf d = 7 Then
        SerialWrite("Sun")
    Else
        SerialWrite("???")
    End If
End Sub

' Prints the copy made by the last RtccRead: 2026-10-31 Sat 14:30:00
Sub PrintDateTime()
    SerialWrite("20")
    Print2(RtccYear())
    SerialWrite("-")
    Print2(RtccMonth())
    SerialWrite("-")
    Print2(RtccDay())
    SerialWrite(" ")
    PrintDayName(RtccWeekday())
    SerialWrite(" ")
    Print2(RtccHour())
    SerialWrite(":")
    Print2(RtccMinute())
    SerialWrite(":")
    Print2(RtccSecond())
    SerialNewLine()
End Sub

Sub PrintStatus(ByVal st As Byte)
    SerialWrite("ERR ")
    SerialWriteByte(48 + st)              ' status codes are 0..6
    SerialNewLine()
End Sub

Sub PrintHelp()
    SerialWrite("S yymmddhhmmss  set date and time")
    SerialNewLine()
    SerialWrite("T hhmmss        set time")
    SerialNewLine()
    SerialWrite("D yymmdd        set date")
    SerialNewLine()
End Sub

' ---------------------------------------------------------------------------------------------
' Calendar helpers (years 2000..2099)
' ---------------------------------------------------------------------------------------------

Function DaysInMonth(ByVal y As Byte, ByVal m As Byte) As Byte
    Result = 31
    If m = 4 Or m = 6 Or m = 9 Or m = 11 Then
        Result = 30
    ElseIf m = 2 Then
        Result = 28
        ' 2000 is a leap year and 2100 is out of range, so every fourth year is a leap year.
        If (y And 3) = 0 Then
            Result = 29
        End If
    End If
End Function

' 1 = Monday .. 7 = Sunday.
Function CalcWeekday(ByVal y As Byte, ByVal m As Byte, ByVal d As Byte) As Byte
    Dim t As Byte
    ' Sakamoto's formula, y + y/4 + offset + day (mod 7), for 2000..2099 with Jan and Feb counted in
    ' the previous year. The century terms add 2485 = 7 * 355, so they drop out. January and
    ' February 2000 count as 1999, which is worth 5. The largest sum is 160, so a Byte holds it
    ' and no 16-bit divide is needed.
    If m < 3 And y = 0 Then
        t = 5
    ElseIf m < 3 Then
        t = y - 1
        t = t + (t >> 2)
    Else
        t = y + (y >> 2)
    End If
    t = t + MonthOffset(m - 1) + d
    While t >= 7                          ' t Mod 7 without the 196-word Byte divide routine
        t = t - 7
    Wend
    Result = t                            ' 0 = Sunday
    If Result = 0 Then
        Result = 7
    End If
End Function

' ---------------------------------------------------------------------------------------------
' Command line
' ---------------------------------------------------------------------------------------------

' True when cmd(first .. first + count - 1) are all decimal digits.
Function Digits(ByVal first As Byte, ByVal count As Byte) As Boolean
    Dim i As Byte
    Result = True
    For i = first To first + count - 1
        If cmd(i) < 48 Or cmd(i) > 57 Then
            Result = False
        End If
    Next
End Function

' The two-digit number at cmd(i), cmd(i + 1).
Function Num2(ByVal i As Byte) As Byte
    Result = (cmd(i) - 48) * 10 + (cmd(i + 1) - 48)
End Function

' Checks the parsed fields, then loads them into the chip. Prints OK or the reason.
Sub ApplyTime()
    Dim st As Byte
    If fm < 1 Or fm > 12 Or fd < 1 Or fh > 23 Or fmi > 59 Or fs > 59 Then
        SerialWrite("ERR range")
        SerialNewLine()
        Exit
    End If
    If fd > DaysInMonth(fy, fm) Then
        SerialWrite("ERR range")
        SerialNewLine()
        Exit
    End If
    st = RtccSet(fy, fm, fd, CalcWeekday(fy, fm, fd), fh, fmi, fs)
    If st = 0 Then
        SerialWrite("OK")
        SerialNewLine()
    Else
        PrintStatus(st)
    End If
End Sub

' Runs the finished line in cmd(0 .. cmdLen - 1).
Sub RunCommand()
    Dim st As Byte
    Dim p As Byte
    Dim c As Byte
    c = cmd(0)
    If c >= 97 And c <= 122 Then          ' lower case -> upper case
        c = c - 32
    End If
    p = 1
    If cmdLen > 1 And cmd(1) = 32 Then    ' an optional space after the letter
        p = 2
    End If
    If c = 63 Then                        ' '?'
        PrintHelp()
    ElseIf c = 83 And cmdLen = p + 12 And Digits(p, 12) Then
        fy = Num2(p)
        fm = Num2(p + 2)
        fd = Num2(p + 4)
        fh = Num2(p + 6)
        fmi = Num2(p + 8)
        fs = Num2(p + 10)
        ApplyTime()
    ElseIf c = 84 And cmdLen = p + 6 And Digits(p, 6) Then
        st = RtccRead()                   ' keep the current date
        If st = 0 Then
            fy = RtccYear()
            fm = RtccMonth()
            fd = RtccDay()
            fh = Num2(p)
            fmi = Num2(p + 2)
            fs = Num2(p + 4)
            ApplyTime()
        Else
            PrintStatus(st)
        End If
    ElseIf c = 68 And cmdLen = p + 6 And Digits(p, 6) Then
        st = RtccRead()                   ' keep the current time
        If st = 0 Then
            fy = Num2(p)
            fm = Num2(p + 2)
            fd = Num2(p + 4)
            fh = RtccHour()
            fmi = RtccMinute()
            fs = RtccSecond()
            ApplyTime()
        Else
            PrintStatus(st)
        End If
    Else
        SerialWrite("? for help")
        SerialNewLine()
    End If
End Sub

' Takes every received byte; a carriage return or line feed ends the line.
Sub ServiceSerial()
    Dim c As Byte
    While SerialAvailable()
        c = SerialRead()
        If c = 13 Or c = 10 Then
            If cmdLen > 0 And Not cmdBad Then
                RunCommand()
            ElseIf cmdBad Then
                SerialWrite("ERR too long")
                SerialNewLine()
            End If
            cmdLen = 0
            cmdBad = False
        ElseIf cmdLen < LineMax Then
            cmd(cmdLen) = c
            Inc(cmdLen)
        Else
            cmdBad = True
        End If
    Wend
End Sub

' ---------------------------------------------------------------------------------------------
' Main program
' ---------------------------------------------------------------------------------------------

Dim rst As Byte
Dim lastSec As Byte
Dim errTick As Byte
Dim pollTick As Byte

GpioAllDigital()
SerialInit(Baud)
cmdLen = 0
cmdBad = False
lastSec = 255
errTick = 0
pollTick = 0

SerialNewLine()
SerialWrite("MCP7940N clock/calendar")
SerialNewLine()

If I2cInit(I2cHz, SclPin, SdaPin) = 0 Then
    SerialWrite("I2C init failed")
    SerialNewLine()
End If

' First look at the chip: is it there, has it ever been set, did the power fail?
rst = RtccRead()
If rst = 0 Then
    If Not RtccOscRun() Then
        SerialWrite("Clock was stopped: loading default time")
        SerialNewLine()
        rst = RtccSet(DefYear, DefMonth, DefDay, DefWeekday, 0, 0, 0)
        If rst <> 0 Then
            PrintStatus(rst)
        End If
    ElseIf RtccPowerFail() Then
        SerialWrite("Power failed since last set; time may be wrong")
        SerialNewLine()
    End If
    If UseBattery = 1 Then
        rst = RtccBattery(True)        ' also clears the power-fail flag
    End If
    PrintHelp()
Else
    SerialWrite("RTCC not answering, status ")
    SerialWriteByte(48 + rst)
    SerialNewLine()
End If

' The serial port is serviced every 10 ms, but the bit-banged bus is only used every tenth pass:
' a read takes about a millisecond and the seconds only change once a second.
Do
    If pollTick = 0 Then
        rst = RtccRead()
        If rst = 0 Then
            errTick = 0
            If RtccSecond() <> lastSec Then   ' print once per second
                lastSec = RtccSecond()
                PrintDateTime()
            End If
        Else
            Inc(errTick)
            If errTick >= 10 Then             ' about every second
                errTick = 0
                SerialWrite("RTCC error, status ")
                SerialWriteByte(48 + rst)
                SerialNewLine()
            End If
        End If
    End If
    Inc(pollTick)
    If pollTick >= 10 Then
        pollTick = 0
    End If
    ServiceSerial()
    DelayMs(10)
Loop
