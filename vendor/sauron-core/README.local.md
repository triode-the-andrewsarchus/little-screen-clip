# Local Sauron core compatibility patch

Source: crates.io `sauron-core` 0.61.9, MIT, https://github.com/ivanceras/sauron.
The library source, package manifest and upstream README are retained. Upstream
license is included.

Local change: relax the exact futures 0.3.30 dependency to the workspace-compatible
0.3.34 release. This is needed by SvgBob’s static SVG node writer. No DOM or browser
features are enabled, and no web engine is embedded. An elided return lifetime in `vdom::attribute::group_values` is spelled
explicitly for current Rust’s lifetime-syntax diagnostic (upstream denies all
warnings). No runtime behavior is changed. Validate through the native diagram renderer and workspace checks.
