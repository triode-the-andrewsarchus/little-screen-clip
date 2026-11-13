# Native D2 integration patches

Published source: d2-rs-wasm 0.1.7 and d2-rs-scenebuild 0.1.7 from crates.io,
with the published `.cargo_vcs_info.json` retained. Corresponding original
source archives:

- https://crates.io/api/v1/crates/d2-rs-wasm/0.1.7/download
- https://crates.io/api/v1/crates/d2-rs-scenebuild/0.1.7/download

All changes to these crates remain MPL-2.0. Original source files, notices and
package manifests are retained. Root upstream LICENSE, THIRD_PARTY_NOTICES.md
and SOURCE.md were obtained from the upstream repository; distribution notices
also include the locked added dependencies and bundled font attributions under
`crates/little-clip-render/third_party/d2-notices.txt`.

Local changes:

- The wasm facade exposes its existing source-layout type and a Rust entrypoint
  returning ordinary errors. Native hosts do not construct JavaScript exceptions
  or run JavaScript/WASM. The parser, measurement and layout implementations are
  unchanged.
- The facade builds an rlib only, avoiding an unused shared-library artifact.
- Both manifests accept compatible wasm-bindgen releases instead of pinning
  0.2.128, preserving the workspace's existing 0.2.130 binding family. The scene
  builder's Rust implementation is unchanged.
- Each vendor package declares its own empty workspace to keep upstream tests
  and examples out of this application's workspace test/build targets.

The host adapter uses the native scene builder with an explicit unavailable-image
resolver and bundled-only font fallback. It never invokes the CLI exporter or
its unrestricted filesystem/HTTP/system-font resolvers. Asset-file hydration is
not called. Imports have no host opener; only owned source text is passed in.
