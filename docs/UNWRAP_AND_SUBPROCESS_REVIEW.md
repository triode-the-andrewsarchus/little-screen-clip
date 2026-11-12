# Unwrap and subprocess review

Work continues on `refactor/idiomatic-rust-review`, starting at `647da6d`.

## Method and scope

Inventory first-party Rust source throughout the workspace and supporting Rust helpers. Separate test-only modules and fixture tools from application code using syntax-aware `cfg` inspection and a production Clippy `unwrap_used` pass. Review conditional/platform code from source too. Calls in tests intentionally fail the test when a fixture or assertion prerequisite fails; do not turn those failures into apparently passing tests.

Subprocess review includes both direct `Command::new` and shared `CommandRequest::new` constructors. Merely wrapping a subprocess does not remove its executable dependency. Preserve isolation/sandbox helpers, guarded descriptors, accepted-write completion, explicit external-editor handoffs and platform integration contracts.

## Changes and validation

- [x] Replace both production `sha256sum` uses with core's streaming `FileSource::sha256`. Read the retained ordinary-file handle without changing its cursor, validate its observed source before/after hashing, and poll cancellation/elapsed time between 32 KiB reads. SHA-256 was already a core dependency. This removes the executable dependency and filename-output parsing. Native reads cannot be interrupted mid-system-call; the elapsed limit is cooperative, as documented on the API. Tests cover known digest, empty/multi-chunk input, seek position, unusual filename, cancellation, timeout and stale source rejection.

- [x] Make component-gallery construction fallible. WAV creation/writes/flush and PDF fixture creation now return errors to the existing startup status UI rather than panicking. Test-only callers explicitly require successful fixture setup. A child-process regression test supplies an unusable temporary directory without changing environment variables in concurrent tests. Validation: 17 gallery tests pass; native checksum validation passed two core tests and five guarded pipeline tests.

- [x] Replace three `niri msg` launches with a bounded native Unix-socket client, using existing Rustix/Serde dependencies. [Niri’s IPC documentation](https://docs.rs/niri-ipc/latest/niri_ipc/) specifies newline-framed JSON requests and result-wrapped responses. Preserve the 250 ms total deadline, bounded response bytes, fixed popup geometry, PID/window-size matching and fallback behavior. A full connection backlog fails promptly. Validate reply framing, split packets, application errors, malformed/oversized/incomplete responses, trickle deadlines and window/output payloads (nine popup tests). Read-only queries against the live compositor also returned the documented response variants.

- [x] Report poisoned clipboard-selection state through the read result and tolerate poisoned one-shot acknowledgement ownership during teardown. Recovery only takes an optional sender; it does not revive a selection or change clipboard routing. Validation: four private-compositor/selection tests pass, including the poisoned-reader regression.

- [x] Replace the system `optipng` program with compiled [OxiPNG](https://docs.rs/oxipng/10.2.1/oxipng/), with CLI/parallel/Zopfli features disabled. Keep optimization in a cancellable host helper, shared by both desktop applications. It preserves hidden RGB and text metadata, never increases output size, enforces the shared raster limits and a 30-second optimization budget, and installs only into an unused output through a retained ordinary-directory handle. Errors reach the editor through the existing bounded error-file handoff. Validation: four conversion-worker tests and both applications’ worker-dispatch integration tests, including invalid-input/no-replacement behavior; strict workspace Clippy passed before the final error-message handoff change.

## Remaining review

- [ ] Finish per-site production unwrap decisions, including helper code and deliberate invariants.
- [ ] Finish external-command decisions and practical native replacements.
- [ ] Run final workspace validation and architecture comparison.
