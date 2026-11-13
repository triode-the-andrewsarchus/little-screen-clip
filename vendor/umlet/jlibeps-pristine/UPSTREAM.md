# Pristine source for canonical UMLet's jlibeps 0.1 dependency

- Original project: http://jlibeps.sourceforge.net/
- Source archive published by Debian: https://deb.debian.org/debian/pool/main/j/jlibeps/jlibeps_0.1.ds3.orig.tar.gz
- Archive SHA-256: `7f4a50002b5f7657fff7997a42c8308b03b54f0f0f3abb641b5ecddc949f4a41`
- Retrieved: 2026-11-13.

The archive is retained unchanged (19,248 bytes), with a verbatim copy of its license.txt for inspection. It contains all three library Java sources and build.xml. Their headers declare GPL-2.0-or-later. No local source adaptations are made.

UMLet's official standalone ZIP already contains jlibeps-0.1.jar (17,486 bytes); its legacy all-in-one sequence/activity elements instantiate EpsGraphics2D for an internal layout prepaint even when exporting SVG. The fixed companion retains this tiny dependency to preserve canonical measurements. It exposes no EPS export, filename or output-stream selection to authored source. The Maven 0.1 sources JAR contains only a README, so this upstream-source archive is retained separately rather than claiming the empty Maven artifact supplies source.
