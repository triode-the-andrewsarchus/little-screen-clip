# Bytefield-SVG and bundled ClojureScript runtime

Bytefield-SVG 1.11.0, Copyright James Elliott and contributors, Eclipse Public
License 2.0. Unmodified npm `lib.js`, authoring sources and build descriptors are
retained under `vendor/bytefield-svg`. Its original license follows in this
combined notice. The bytefield library and its accompanying source remain under
that license; the application adapter is separately authored Rust/JavaScript.

Original source and build instructions:
https://github.com/Deep-Symmetry/bytefield-svg/tree/e43dc7037be58772e78f910c3edc8a9b269c8dbd
npm package: https://registry.npmjs.org/bytefield-svg/-/bytefield-svg-1.11.0.tgz
Package SHA-256: 811552e58e93d1106cb634ddeec1f68db4e6371f9edc56cf1787b82202478e80
The bundle and each retained package file are byte-for-byte upstream copies.
No Node CLI, module dependencies for that CLI, or browser host is used.

The upstream compiled bundle includes the ClojureScript ecosystem and Closure
Library runtime. Upstream dependency/build descriptors are retained alongside
its sources. Original EPL-1.0 notices for SCI, ClojureScript and tools.reader,
and the Apache-2.0 Closure Library license follow in this combined notice.

Source availability and attribution:

- SCI (Small Clojure Interpreter), Copyright Michiel Borkent and contributors,
  EPL-1.0: https://github.com/babashka/sci (upstream pins revision 26944ee9e1349fecd113ac6075bd66306ce3a6f8).
- ClojureScript, Copyright Rich Hickey and contributors, EPL-1.0:
  https://github.com/clojure/clojurescript
- tools.reader, Copyright Rich Hickey and contributors, EPL-1.0:
  https://github.com/clojure/tools.reader (upstream declares 1.3.6).
- Analemma 1.1.0, Copyright David Liebke and contributors, EPL-1.0:
  https://clojars.org/analemma/analemma/versions/1.1.0
  Source artifact: https://repo.clojars.org/analemma/analemma/1.1.0/analemma-1.1.0.jar
  (the original Clojure/ClojureScript source is included in that artifact).
- Google Closure Library, Copyright The Closure Library Authors, Apache-2.0:
  https://github.com/google/closure-library

The compiled upstream renderer is embedded as source text and executes only
inside a fresh, bounded, hostless QuickJS runtime. No filesystem, network,
module loader, DOM or JavaScript host bindings are installed. Diagram source
is passed as data to upstream SCI, never concatenated into a JavaScript program.
