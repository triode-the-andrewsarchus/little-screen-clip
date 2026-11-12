# Unwrap and subprocess review

Work continues on `refactor/idiomatic-rust-review`, starting at `647da6d`.

## Method and scope

Inventory first-party Rust source throughout the workspace and supporting Rust helpers. Separate test-only modules and fixture tools from application code using syntax-aware `cfg` inspection and a production Clippy `unwrap_used` pass. Review conditional/platform code from source too. Calls in tests intentionally fail the test when a fixture or assertion prerequisite fails; do not turn those failures into apparently passing tests.

Subprocess review includes both direct `Command::new` and shared `CommandRequest::new` constructors. Merely wrapping a subprocess does not remove its executable dependency. Preserve isolation/sandbox helpers, guarded descriptors, accepted-write completion, explicit external-editor handoffs and platform integration contracts.

## Changes and validation

- [x] Replace both production `sha256sum` uses with core's streaming `FileSource::sha256`. Read the retained ordinary-file handle without changing its cursor, validate its observed source before/after hashing, and poll cancellation/elapsed time between 32 KiB reads. SHA-256 was already a core dependency. This removes the executable dependency and filename-output parsing. Native reads cannot be interrupted mid-system-call; the elapsed limit is cooperative, as documented on the API. Tests cover known digest, empty/multi-chunk input, seek position, unusual filename, cancellation, timeout and stale source rejection.

## Remaining review

- [ ] Finish per-site production unwrap decisions, including helper code and deliberate invariants.
- [ ] Finish external-command decisions and practical native replacements.
- [ ] Run final workspace validation and architecture comparison.
