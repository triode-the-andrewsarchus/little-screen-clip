# Spark Rust source provenance and local patch

The Rust workspace and root MIT license were copied byte-for-byte from
https://github.com/sparkjsdev/spark/tree/f5368253780bed4d863c96b1fdea9dd3d614d131
and committed before local modifications. Only `spark-lib` is linked; the
original CLI/WASM crates and build scripts are retained as source, not run or
added to the application workspace. The upstream workspace Cargo.lock remains
untracked according to the repository's existing lockfile policy.

The local SOG patch replaces only the root metadata's `serde(untagged)` schema
selection with direct V2-then-V1 JSON parsing. It retains the original schema
priority and decoding algorithms. This avoids Serde's buffered fractional-number
failure when another workspace dependency enables serde_json/arbitrary_precision.
Drawing documents keep exact-number preservation. Existing application SOG tests
exercise both metadata versions and all supported spherical-harmonic bands with
fractional values, color, scale, orientation and malformed archive rejection.

The selected-feature build also guards the gzip magic constant with its SPZ
feature and assigns detection completion in each format branch. These two
warning fixes preserve the existing detection flow, including incomplete gzip
headers, without enabling unused decoders or suppressing diagnostics.
