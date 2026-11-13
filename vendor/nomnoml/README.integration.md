# Nomnoml renderer provenance

The preceding pristine-source commit imports these unmodified npm release files:
`LICENSE`, `README.md`, `package.json` and `dist/nomnoml.js` from Nomnoml 1.7.0,
and the corresponding files including `dist/graphre.js` from Graphre 0.1.3.
The engine files have not been patched. Browser UI, CLI and development tooling
are deliberately absent. Both distributions retain their original MIT licenses.

| Package | Release archive | SHA-256 of archive |
| --- | --- | --- |
| Nomnoml 1.7.0 | <https://registry.npmjs.org/nomnoml/-/nomnoml-1.7.0.tgz> | `2c868bb3cc03b58400798121f3c615fa5794fa8ac118302c9724c4df20a9a55e` |
| Graphre 0.1.3 | <https://registry.npmjs.org/graphre/-/graphre-0.1.3.tgz> | `8bab988c253a21a3f488d3eeba768e9c94e6ee9c8b94e5f69c8d910a0df0a3c8` |

Each archive was also checked against its npm SHA-512 integrity value before
extraction, and every imported file was compared byte-for-byte with that release.
Upstreams: <https://github.com/skanaar/nomnoml> and
<https://github.com/skanaar/graphre>.

This import alone does not enable the format. Integration must use the existing
bounded hostless interpreter and SVG composition, with no installed Node runtime,
DOM, module loader or filesystem access. Call only the data-taking `renderSvg`
entrypoint; do not expose `compileFile`, import loaders or source evaluation.
Import directives need a visible refusal rather than silently incomplete output.
The server renderer uses estimated character widths without a document canvas;
rendering and font-layout checks must cover this limitation before shipping.

A development-only C probe linked the already-built rquickjs-sys 0.14.0 QuickJS
archive and called `renderSvg` with an actual JS string argument. With 128 MiB
heap, 512 KiB stack and an interrupt deadline, it produced exactly the same
4,375-byte SVG as the Node VM prototype (SHA-256
`290c1ba80aca7936403fc68792a5805b48718d45a1ca2245c36d27f1f410f1b8`).
This proves the bundles can run on the selected interpreter; it does not replace
application worker, resource-limit, rendering or routing tests.
