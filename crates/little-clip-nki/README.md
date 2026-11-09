# Native sampler mapping helper

This crate is an LGPL-3.0-only executable. Application crates communicate with it
using bounded JSON over standard input/output; they do not link its parser.
There is no JVM, Maven, JavaFX, UiTools or Java runtime dependency.

The read-only parser is adapted from ConvertWithMoss by Jürgen Moßgraber and
contributors, copyright 2019–2026, reviewed revision
`aa89125fec206363cc3cc90ded8487a4acfd3397`:
https://github.com/git-moss/ConvertWithMoss.
Adapted portions are the NI container/chunk/subtree readers, Kontakt preset
structures, zone and filename mapping, legacy headers/paths, FastLZ decoding and EXS block/zone/sample metadata.
The Rust adaptation adds checked slice reads, count/depth/expansion limits and a
small metadata-only protocol. Conversion, scripting and instrument DSP are absent.
`COPYING` and `COPYING.LESSER` contain the license texts.
`THIRD_PARTY_NOTICES` retains the original FastLZ attribution and MIT notice
from the Java reader's documented upstream provenance. The modern format fixture
`tests/fixtures/kontakt680.nki` is upstream's LGPL Kontakt680Template.nki, retained
here for regression testing with its source attribution; its referenced samples
are not included. Other fixtures belong to the application test suite.

Build and package with `python3 scripts/build_multisample_helper.py`. This installs
`target/release/nki-helper/little-clip-nki`, source, a standalone Cargo manifest,
the lockfile and licenses. Distribute the directory intact. Its source can be
rebuilt independently with `cargo build --release --manifest-path source/Cargo.toml`
from that directory; replace `little-clip-nki` with the resulting executable.
`LITTLE_CLIP_NKI_HELPER` selects another compatible helper directory. An ordinary
workspace build also builds the executable; packaging supplies its source/notices.

The application runs the helper with Bubblewrap, prlimit, no network or access to
the user's files, a 512 MiB address-space budget, 15 seconds of CPU time, 20 seconds
elapsed time, 64 MiB input, 16 MiB expanded payloads and 4 MiB output. Sample paths
remain references. Only the application's guarded reader opens a selected sample.

Supported mappings: legacy compressed XML instruments, Kontakt 4.2 binary presets,
readable Kontakt 5+ containers, and little/big-endian Logic EXS instruments. EXS headers, declared inventory counts and zone/sample associations are validated. Protected/encrypted presets, legacy monolith
sample tables, embedded sample extraction and instrument effects/scripts are not
implemented. A raw sample excerpt is a preview of the source recording, not a
reproduction of Kontakt's full instrument behavior.

External NCW recordings are decoded by the application's separate MIT/Apache
native audio dependency. This helper only describes their paths and mappings.
