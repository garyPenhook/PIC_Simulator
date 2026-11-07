' Fixture for tests/pic16_lines.rs: the line numbers below are the ones p16lines.S names with #line (the assembly was written by hand;
' basil does not write line information for the enhanced mid-range core yet).
Program Lines
Device = 16F17146
Dim a As Byte
a = 5
Call Sub1
a = 7
End
Sub Sub1()
    a = 1
End Sub
