; Owned frame toolpath: mm, two heights and a rounded corner
G21 G90
M82
G92 E0
G0 X5 Y5 Z0.2
G1 X45 Y5 E1 F1200
G1 X45 Y35 E2
G1 X5 Y35 E3
G1 X5 Y5 E4
G0 X5 Y5 Z0.4
G1 X35 Y5 E5
G3 X45 Y15 I0 J10 E6
G1 X45 Y35 E7
G1 X5 Y35 E8
G1 X5 Y5 E9
M2
G1 X999 Y999 ; Must not appear after program end
