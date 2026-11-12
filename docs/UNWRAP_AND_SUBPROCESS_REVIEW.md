# Unwrap and subprocess review

Work continues on `refactor/idiomatic-rust-review`, starting at `647da6d`.

## Method and scope

Inventory Rust source throughout the workspace, maintained vendor forks and supporting Rust helpers (not downloaded registry dependencies). Separate test-only modules and fixture tools from application code using syntax-aware `cfg` inspection and a production Clippy `unwrap_used` pass. Review conditional/platform code from source too. Calls in tests intentionally fail the test when a fixture or assertion prerequisite fails; do not turn those failures into apparently passing tests.

Subprocess review includes both direct `Command::new` and shared `CommandRequest::new` constructors. Merely wrapping a subprocess does not remove its executable dependency. Preserve isolation/sandbox helpers, guarded descriptors, accepted-write completion, explicit external-editor handoffs and platform integration contracts.

## Changes and validation

- [x] Replace both production `sha256sum` uses with core's streaming `FileSource::sha256`. Read the retained ordinary-file handle without changing its cursor, validate its observed source before/after hashing, and poll cancellation/elapsed time between 32 KiB reads. SHA-256 was already a core dependency. This removes the executable dependency and filename-output parsing. Native reads cannot be interrupted mid-system-call; the elapsed limit is cooperative, as documented on the API. Tests cover known digest, empty/multi-chunk input, seek position, unusual filename, cancellation, timeout and stale source rejection.

- [x] Make component-gallery construction fallible. WAV creation/writes/flush and PDF fixture creation now return errors to the existing startup status UI rather than panicking. Test-only callers explicitly require successful fixture setup. A child-process regression test supplies an unusable temporary directory without changing environment variables in concurrent tests. Validation: 17 gallery tests pass; native checksum validation passed two core tests and five guarded pipeline tests.

- [x] Replace three `niri msg` launches with a bounded native Unix-socket client, using existing Rustix/Serde dependencies. [Niri’s IPC documentation](https://docs.rs/niri-ipc/latest/niri_ipc/) specifies newline-framed JSON requests and result-wrapped responses. Preserve the 250 ms total deadline, bounded response bytes, fixed popup geometry, PID/window-size matching and fallback behavior. A full connection backlog fails promptly. Validate reply framing, split packets, application errors, malformed/oversized/incomplete responses, trickle deadlines and window/output payloads (nine popup tests). Read-only queries against the live compositor also returned the documented response variants.

- [x] Report poisoned clipboard-selection state through the read result and tolerate poisoned one-shot acknowledgement ownership during teardown. Recovery only takes an optional sender; it does not revive a selection or change clipboard routing. Validation: four private-compositor/selection tests pass, including the poisoned-reader regression.

- [x] Replace the system `optipng` program with compiled [OxiPNG](https://docs.rs/oxipng/10.2.1/oxipng/), with CLI/parallel/Zopfli features disabled. Keep optimization in a cancellable host helper, shared by both desktop applications. It preserves hidden RGB and text metadata, never increases output size, enforces the shared raster limits and a 30-second optimization budget, and installs only into an unused output through a retained ordinary-directory handle. Errors reach the editor through the existing bounded error-file handoff. Validation: four conversion-worker tests and both applications’ worker-dispatch integration tests, including invalid-input/no-replacement behavior; strict workspace Clippy passed before the final error-message handoff change.

- [x] Handle invalid/overflowing film-grain variance in the maintained `filmr` fork. A failed normal-distribution construction now contributes zero additional noise, consistent with the existing zero-variance path. Tests cover infinite and overflowing noise parameters.

- [x] Replace both `prlimit` wrappers with native child resource limits in core’s existing `CommandRequest` pre-exec setup. Preserve each caller’s exact address-space, CPU and file-size budgets before Bubblewrap or a replaceable helper runs. The existing audited hook performs only syscalls, using vectors captured before fork, and never changes parent limits. Validation: seven process tests, including a parent with a large live virtual allocation; four native XCF tests and the normally ignored packaged NKI sandbox test pass under the new launcher.

- [x] Replace `blender-thumbnailer` with a bounded native saved-preview reader in core, sharing existing gzip/Zstd libraries. Read only REND/TEST prefix blocks, support legacy endian/pointer layouts and Blender 5 headers, cap compressed/decoded prefix reads at 8 MiB, the decoder window at 32 MiB and thumbnails at 1024 pixels per edge, and retain source checks/cancellation. [Blender’s header definitions](https://github.com/blender/blender/blob/main/source/blender/blenloader_core/BLO_core_blend_header.hh) and [block layout](https://github.com/blender/blender/blob/main/source/blender/blenloader_core/BLO_core_bhead.hh) define the wire format; the implementation is independent. A whole-scene parser such as [`blend`](https://docs.rs/blend/latest/blend/) is unnecessary and documents trusted-file assumptions. Four tests cover header variants, compression, sparse large files, malformed/truncated inputs and budgets. A downloaded [Assimp test scene](https://raw.githubusercontent.com/assimp/assimp/master/test/models/BLEND/SuzanneSubdiv_252.blend) also matches the installed thumbnailer pixel-for-pixel (the optional reference test was run explicitly). No scene rendering or scripts are executed.

- [x] Launch the native PNG optimizer through the existing command builder with parent-death ownership and hard child limits (2 GiB address space, 35 CPU seconds, 128 MiB staged-file size). Its existing cancellation/process-group owner still kills and reaps the child. Both host integration suites pass with the final launcher.

## Remaining review

- [ ] Finish per-site production unwrap decisions, including helper code and deliberate invariants.
- [ ] Finish external-command decisions and practical native replacements.
- [ ] Run final workspace validation and architecture comparison.
