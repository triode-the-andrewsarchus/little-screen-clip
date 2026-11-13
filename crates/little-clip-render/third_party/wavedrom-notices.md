# WaveDrom engine and parser provenance

The WaveDrom 3.8.0 npm package is retained unmodified under `vendor/wavedrom`.
The unminified `wavedrom.unpkg.js` bundle and five additional official skins are
embedded in the application. The default skin is already in that bundle.
The browser helpers in the published package are never called. No Node runtime,
DOM, web engine, source evaluation, dynamic module loader or source-driven I/O
is provided. Only `renderAny` and the bundled SVG serializer are used.

Original sources and package:

- https://github.com/wavedrom/wavedrom/tree/5f44f7d836c550fa1f0fff2ea23cc95a5c0695e3
- https://registry.npmjs.org/wavedrom/-/wavedrom-3.8.0.tgz
- npm SHA-512 integrity: `sha512-nwYKmpoeRAvtkbVb7bstjzW+fxjLnaHM6tYjex41o5Tew1VLf0dN/gUcKGJGfPS3uDkbLdXRas3BKiDZ32GLkA==`

All 53 published files were byte-compared with the extracted package after
verifying the archive integrity. The import commit is separate from application
integration. No upstream JavaScript is patched. The package and its bundled
onml, tspan, logidrom and bit-field components use MIT licenses, reproduced beside
this notice. The package declares onml ^2.3.0, tspan ^0.4.0, logidrom ^1.2.1 and
bit-field ^1.11.0; notices are from those releases. Published bundles do not ship
a dependency lockfile, so exact resolved versions are not inferred from ranges.

QuickJS is compiled into the application through rquickjs 0.14.0, with only its
Rust std feature. Both the Rust bindings' MIT license and the QuickJS engine's
MIT license accompany this notice. Sources:

- https://crates.io/api/v1/crates/rquickjs/0.14.0/download
- https://crates.io/api/v1/crates/rquickjs-sys/0.14.0/download
- https://github.com/DelSkayn/rquickjs

JSON5 1.3.7 supplies data deserialization; json-five 0.3.1 supplies a Rust lexer
with source positions for preflight and later source correspondence. Their MIT
licenses accompany this notice. Sources:

- https://crates.io/api/v1/crates/json5/1.3.7/download
- https://crates.io/api/v1/crates/json-five/0.3.1/download

SVG text reuses the application's existing CJK/emoji and OFL Latin metric faces;
no upstream Helvetica font asset is imported. Generated SVG is outlined through
the existing guarded SVG pipeline before publication/export.
