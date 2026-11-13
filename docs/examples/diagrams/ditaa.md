# Ditaa previews

Local source and rendered views of the same diagrams.

## pipeline.ditaa

```ditaa
+------------+     +------------+     +------------+
| Source     |---->| Local      |---->| Preview    |
| cBLU {d}   |     | Ditaa      |     | cGRE       |
+------+-----+     +------------+     +------------+
       :                                    ^
       |         bounded SVG output         |
       +------------------------------------+
```

## shapes.ditaa

```ditaa
+----------+ +----------+ +----------+ +----------+
|Doc       | |Store     | |I/O       | |Oval      |
|cBLU {d}  | |cGRE {s}  | |cYEL {io} | |cPNK {o}  |
+----------+ +----------+ +----------+ +----------+

+----------+ +----------+ +----------+
|Manual    | |Choice    | |Trap      |
|{mo}      | |{c}       | |{tr}      |
+----------+ +----------+ +----------+

/------------+------------\
| cRED       | cBLU       |
+------------+------------+
| cGRE       | cPNK       |
\------------+------------/

*----*     +=====> dashed
|    |
*----*
```
