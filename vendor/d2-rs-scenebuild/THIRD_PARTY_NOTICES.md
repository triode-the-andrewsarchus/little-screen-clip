# Third-party notices

d2-rs's first-party code is licensed under MPL-2.0. Third-party portions
retain the terms below; Cargo dependency notices are generated into DEPENDENCY_NOTICES.md by `node scripts/sync-licenses.mjs`.

## D2 and TALA

Copyright 2022 Terrastruct Inc.

TALA's runtime source was published under D2's MPL-2.0 license in
https://github.com/d2lang/d2/pull/2882 (merge commit 99b0791).
The upstream TALA authors list recognizes Alexander Wang, Gavin Nishizawa,
and Júlio César Batista.

The layout pipeline and D2-derived geometry are distributed under MPL-2.0;
the complete license is in LICENSE. Rust translation and subsequent
modifications: Copyright 2026 Nelson Chen.

The OOXML base package in `crates/d2-rs-export/assets/d2-template.pptx` is the
v0.9.0 D2 PowerPoint template from `lib/pptx/template.pptx`. It is retained
under D2's MPL-2.0 terms; the pinned D2 reference checkout is the corresponding
source.

The live-preview client in `crates/d2-rs-cli/src/watch_static/` is copied from
the D2 v0.9.0 `d2cli/static/` watch assets and remains under D2's MPL-2.0 terms.
The corresponding source is the pinned D2 reference checkout.

## Dagro, Dagre, and Graphlib

`crates/d2-rs-dagre` translates the ranking, compound ordering, and
Brandes–Köpf placement algorithms from `github.com/d2lang/dagro v0.2.1`,
the dependency pinned by D2 v0.9.0. Dagro is derived from Dagre 3.1.1 and
Graphlib. These portions retain the following MIT notice; Rust additions
and modifications are distributed under MPL-2.0.

```text
MIT License

Copyright (c) 2012-2014 Chris Pettitt
Copyright (c) 2026 Dagro contributors

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

## MathJax (mathjax-go) translation and TeX font data

`crates/d2-rs-math` contains a Rust translation and modification of
`github.com/d2lang/mathjax-go` at the version upstream D2 pins
(`v0.1.1-0.20261002175158-94d80186bfd7`), itself a Go translation and
modification of MathJax 3.2.2. Copyright 2026 D2 contributors; Copyright (c)
2009–2022 The MathJax Consortium; color support additionally Copyright (c)
2018–2022 Omar Al-Ithawi and The MathJax Consortium. mathjax-go and MathJax are
licensed under the Apache License, Version 2.0
(https://www.apache.org/licenses/LICENSE-2.0).

- **Translated TeX parser and SVG layout:** these files carry an
  `SPDX-License-Identifier: Apache-2.0` header and keep Apache-2.0 terms and
  the MathJax attribution.
- **mhchem** (`crates/d2-rs-math/src/mhchem/`): translates mhchemParser 4.1.1,
  Copyright (c) 2015–2021 Martin Hensel (Apache-2.0), and the initial go-tex
  structure, Copyright (c) 2026 luo-studio and Copyright (c) erweixin (MIT).
  These files are Apache-2.0 AND MIT.
- **Generated font tables:** the glyph metrics and outlines in
  `crates/d2-rs-math/src/glyphs.rs`, the other `*_glyphs.rs` modules,
  `vertical_delimiters.rs` and `compound_glyphs.rs` are source-derived from
  mathjax-go's `internal/font/*_gen.go` (identical between v0.1.0 and the pinned
  version), which are mechanically derived from MathJax 3.2.2 TeX font data.
  They keep Apache-2.0 terms, whether a file's header is MPL-2.0 with an
  Apache-2.0 note or Apache-2.0 alone.
- **First-party code:** files whose header is MPL-2.0 without an Apache-2.0
  source note are first-party d2-rs code under MPL-2.0.

mathjax-go's `LICENSE` and `NOTICE` and its MIT licenses are retained unchanged
in `crates/d2-rs-math/licenses/` (`mathjax-go-LICENSE.txt`,
`mathjax-go-NOTICE.txt`, `mathjax-go-MIT-Luo-Studio.txt`,
`mhchem-go-tex-MIT.txt`).

## Source Sans Pro and Source Code Pro

The native text measurer and raster exporter embed the Source Sans Pro and
Source Code Pro faces from D2 v0.9.0's `d2renderers/d2fonts/ttf` directory,
including regular, semibold, bold, and italic variants where available. The
fonts are Copyright 2010–2018 Adobe Systems Incorporated and are licensed
under the SIL Open Font License, Version 1.1. The complete font license is included at
`crates/d2-rs-text/assets/OFL-1.1.txt` and remains applicable to the font
bytes.

## Fibonacci heap

The heap in crates/d2-rs-tala/src/internal/routing/fibheap.rs
closely corresponds to Workiva/go-datastructures' fibheap/fibheap.go,
including circular-list merging, consolidation, cascading cuts, and
less-than-or-equal decrease-key ties. We preserve its Apache-2.0 notice.
The Rust version replaces pointers with arena indices, adds generic payloads,
and reuses scratch allocations.

Reference reviewed: https://github.com/Workiva/go-datastructures/blob/89d15facb2e3d7ae0c819fa43b66a2153e77196f/fibheap/fibheap.go

Special thanks to Keith Schwarz, whose code and documentation were used as
a reference by that implementation:
http://www.keithschwarz.com/interesting/code/?dir=fibonacci-heap

Licensed under the Apache License, Version 2.0 (the "License");
you may not use this file except in compliance with the License.
You may obtain a copy of the License at
https://www.apache.org/licenses/LICENSE-2.0
Unless required by applicable law or agreed to in writing, software
distributed under the License is distributed on an "AS IS" BASIS,
WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
See the License for the specific language governing permissions and
limitations under the License.

## Go standard library

`crates/d2-rs-export/src/go_png.rs` and `go_zip.rs` translate Go 1.27.0's
RGB/NRGBA8 PNG writer, adaptive filters, buffered IDAT emission and the
reachable ZIP Create/Copy headers, descriptors and directory records from
`image/png/writer.go`, `bufio/bufio.go` and
`archive/zip/{writer,struct,register}.go`. Copyright 2009, 2010 The Go Authors.
All rights reserved. The complete Go BSD-3-Clause terms below apply.
Compression uses the existing `d2-rs-goflate` translation with its retained Go
notices.

The URL decoder in `crates/d2-rs-wasm/src/url_codec.rs` translates Go 1.27.0
`encoding/base64` decodeQuantum and `compress/flate` Huffman/bit-reader rules.
Copyright 2009 The Go Authors. The Go BSD terms below apply.

The WASM host-value adapter in `crates/d2-rs-wasm/src/go-js-string-args.cjs`
follows Go 1.27.0 `syscall/js.Value.String` and `wasm_exec.js` value
marshaling. Copyright 2018 The Go Authors. The Go BSD terms below apply.

`crates/d2-rs-goflate/src/` (`deflate.rs`, `deflatefast.rs`, `level1.rs`
through `level6.rs`, `huffman_bit_writer.rs`, `huffman_code.rs`, `token.rs`,
`zlib.rs`, `adler32.rs`) translates the Go 1.27.0 `compress/flate` encoder,
`compress/zlib` writer and `hash/adler32`. Copyright 2009-2026 The Go Authors.
The Go BSD terms below apply.

Portions of `crates/d2-rs-tala/src/internal/runtime/go_rng.rs`,
`crates/d2-rs-tala/src/internal/runtime/go_sort.rs` and
`crates/d2-rs-core/src/go_sort.rs` (also used by the compiler through
`crates/d2-rs-language/src/compiler/go_sort.rs`) are translated from the Go
1.27.0 standard library's `math/rand` and `sort` packages.

String quoting in `crates/d2-rs-core/src/go_quote.rs` and
`crates/d2-rs-text/src/go_quote.rs` translates Go 1.27.0 `strconv.Quote`, and
the JSON string encoding in `crates/d2-rs-lib/src/lib.rs` follows Go 1.27.0
`encoding/json`'s string escaping.

`crates/d2-rs-tala/src/internal/placement/owner_median.rs` also follows
Go 1.27.0 `src/math/dim.go`, `dim_arm64.s` and `dim_amd64.s` for
`math.Max`, including architecture-specific NaN and signed-zero behavior.

`crates/d2-rs-tala/src/internal/routing/channel_points.rs` also translates
Go 1.27.0 `src/math/hypot.go` (the generic implementation used on ARM64).

The graphjson decoder in `crates/d2-rs-tala/src/internal/graphjson/decode.rs`
uses Go 1.27.0 JSON field-update semantics and translates error formatting from
`src/encoding/json/v2_inject.go` and `v2_decode.go`.

The numeric parser in `crates/d2-rs-language/src/parser/numeric.rs` translates
Go 1.27.0 `src/math/big/ratconv.go` and `natconv.go` string-acceptance and
exponent rules.

The public parser in `crates/d2-rs-language/src/parse.rs` translates the
UTF-16LE decoder from `golang.org/x/text` v0.41.0
`encoding/unicode/unicode.go`. Diagnostic input quoting follows Go 1.27.0
`src/strconv/quote.go`.

D2 scalar floating-point parsing in `crates/d2-rs-core/src/go_float.rs`
translates Go 1.27.0 `src/internal/strconv/{atof,atoi}.go` lexical scanning,
underscore validation and hexadecimal sticky-bit rounding. Decimal conversion
uses Rust's correctly rounded parser after Go syntax validation. These
Go-derived portions retain the Go BSD terms below (Copyright 2009 The Go Authors).

The IR edge matching table in `crates/d2-rs-core/src/ir/fold.rs` is generated
from Go 1.27.0 `unicode.SimpleFold` and its Unicode 17.0.0 tables.

The substitution array backing model in
`crates/d2-rs-language/src/compiler/arrays.rs` translates Go 1.27.0
`src/runtime/slice.go`, `src/runtime/msize.go`, and the allocation constants in
`src/internal/runtime/gc/sizeclasses.go` and `malloc.go`. It preserves the
observable nested-append and range aliasing of D2's `[]Value` slices.

Native file handling in `crates/d2-rs-language/src/stream/native.rs` follows
Go 1.27.0 `src/os/file.go` and `src/internal/poll/fd_unix.go`. Generated
`stream/os_errors.rs` preserves the architecture-specific Go syscall errno
strings for Darwin/Linux ARM64/x86-64, under the Go BSD terms below.

HTML/URL classification in `crates/d2-rs-language/src/compiler/import_urls.rs`
and its generated entity table follow Go 1.27.0 `src/html/{escape,entity}.go`
and the reachable parsing/validation rules of `src/net/url/url.go`,
`gen_encoding_table.go`, and `src/net/netip/netip.go`. The byte path join in
`compiler/import_links.rs` follows `src/path/path.go`. These Go-derived
portions retain the Go BSD terms below.

The streaming parser in `crates/d2-rs-language/src/stream.rs` follows Go 1.27.0
`src/bufio/bufio.go` and `src/unicode/utf8/utf8.go`, plus `golang.org/x/text`
v0.41.0 `encoding/unicode/unicode.go` and `transform/transform.go`, for
buffering, BOM transitions, UTF-16 decoding and read-error behavior.

Public request/font admission in
`crates/d2-rs-wasm/src/public_compile_request.rs` follows D2's MPL-2.0
`d2js/d2wasm`, `d2renderers/d2fonts`, `lib/font` and `lib/textmeasure`
contracts. The font-collection header/error checks follow
`golang.org/x/image v0.45.0/font/sfnt/sfnt.go` (Copyright 2016 The Go
Authors); those Go-derived portions retain the Go BSD terms below.
The Rust validator does not embed a Go runtime or translate the WOFF encoder.

Copyright 2013 The Go Authors. All rights reserved.
Copyright 2015 The Go Authors. All rights reserved.
Copyright 2016 The Go Authors. All rights reserved.
Copyright 2024 The Go Authors. All rights reserved.
Copyright 2025 The Go Authors. All rights reserved.
Copyright 2009 The Go Authors.
Copyright 2010 The Go Authors. All rights reserved.

Redistribution and use in source and binary forms, with or without
modification, are permitted provided that the following conditions are met:

- Redistributions of source code must retain the above copyright notice, this
  list of conditions and the following disclaimer.
- Redistributions in binary form must reproduce the above copyright notice,
  this list of conditions and the following disclaimer in the documentation
  and/or other materials provided with the distribution.
- Neither the name of Google LLC nor the names of its contributors may be used
  to endorse or promote products derived from this software without specific
  prior written permission.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS “AS IS”
AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE
IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE
ARE DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT OWNER OR CONTRIBUTORS BE
LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR
CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF
SUBSTITUTE GOODS OR SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS
INTERRUPTION) HOWEVER CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN
CONTRACT, STRICT LIABILITY, OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE)
ARISING IN ANY WAY OUT OF THE USE OF THIS SOFTWARE, EVEN IF ADVISED OF THE
POSSIBILITY OF SUCH DAMAGE.

`crates/d2-rs-lib/src/urlenc.rs` reuses the native decoder translated from Go 1.27
`encoding/base64/base64.go` and `compress/flate/inflate.go` (Copyright 2009
The Go Authors), with the D2 master decoded-size admission boundary.
The Go BSD-3-Clause terms below apply.

`crates/d2-rs-sketch/src/gomath.rs` translates Go 1.27.0 `src/math/sin.go`,
`trig_reduce.go`, `tan.go`, `atan.go`, `asin.go` and `dim.go` (Cephes-derived
kernels; Copyright 2009-2018 The Go Authors), under the Go BSD terms below.

The native TALA container slice storage in
`crates/d2-rs-tala/src/internal/layoutgraph/container_slices.rs` and the typed
raw incident slice storage in
`crates/d2-rs-tala/src/internal/layoutgraph/owner_pointer_slices.rs` translate
Go 1.27.0 scanned-pointer growth and allocation size classes from
`src/runtime/slice.go`, `src/runtime/msize.go`, and
`src/internal/runtime/gc/{malloc,sizeclasses}.go`. Shared map/header storage
and exact capacity-window rollback follow the pinned D2 TALA source. The local
route result builder in `layoutgraph/route_points.rs` additionally translates
`growsliceBuf` and `moveSlice` from `src/runtime/slice.go`, as selected by
`src/cmd/compile/internal/slice/slice.go` in the pinned compiler.

## Mocha regression input

`crates/d2-rs-cli/tests/fixtures/tala_cluster_obstruction.d2` is reduced from
[lowRISC Mocha](https://github.com/lowRISC/mocha/blob/b5973217f704923917e7761f73df7dfcb8d0c345/doc/img/mocha.d2).
SPDX-FileCopyrightText: lowRISC Contributors (COSMIC project).
It is licensed under Apache-2.0, whose terms are reproduced below.

## Apache License 2.0 (Fibonacci heap)


                                 Apache License
                           Version 2.0, January 2004
                        http://www.apache.org/licenses/

   TERMS AND CONDITIONS FOR USE, REPRODUCTION, AND DISTRIBUTION

   1. Definitions.

      "License" shall mean the terms and conditions for use, reproduction,
      and distribution as defined by Sections 1 through 9 of this document.

      "Licensor" shall mean the copyright owner or entity authorized by
      the copyright owner that is granting the License.

      "Legal Entity" shall mean the union of the acting entity and all
      other entities that control, are controlled by, or are under common
      control with that entity. For the purposes of this definition,
      "control" means (i) the power, direct or indirect, to cause the
      direction or management of such entity, whether by contract or
      otherwise, or (ii) ownership of fifty percent (50%) or more of the
      outstanding shares, or (iii) beneficial ownership of such entity.

      "You" (or "Your") shall mean an individual or Legal Entity
      exercising permissions granted by this License.

      "Source" form shall mean the preferred form for making modifications,
      including but not limited to software source code, documentation
      source, and configuration files.

      "Object" form shall mean any form resulting from mechanical
      transformation or translation of a Source form, including but
      not limited to compiled object code, generated documentation,
      and conversions to other media types.

      "Work" shall mean the work of authorship, whether in Source or
      Object form, made available under the License, as indicated by a
      copyright notice that is included in or attached to the work
      (an example is provided in the Appendix below).

      "Derivative Works" shall mean any work, whether in Source or Object
      form, that is based on (or derived from) the Work and for which the
      editorial revisions, annotations, elaborations, or other modifications
      represent, as a whole, an original work of authorship. For the purposes
      of this License, Derivative Works shall not include works that remain
      separable from, or merely link (or bind by name) to the interfaces of,
      the Work and Derivative Works thereof.

      "Contribution" shall mean any work of authorship, including
      the original version of the Work and any modifications or additions
      to that Work or Derivative Works thereof, that is intentionally
      submitted to Licensor for inclusion in the Work by the copyright owner
      or by an individual or Legal Entity authorized to submit on behalf of
      the copyright owner. For the purposes of this definition, "submitted"
      means any form of electronic, verbal, or written communication sent
      to the Licensor or its representatives, including but not limited to
      communication on electronic mailing lists, source code control systems,
      and issue tracking systems that are managed by, or on behalf of, the
      Licensor for the purpose of discussing and improving the Work, but
      excluding communication that is conspicuously marked or otherwise
      designated in writing by the copyright owner as "Not a Contribution."

      "Contributor" shall mean Licensor and any individual or Legal Entity
      on behalf of whom a Contribution has been received by Licensor and
      subsequently incorporated within the Work.

   2. Grant of Copyright License. Subject to the terms and conditions of
      this License, each Contributor hereby grants to You a perpetual,
      worldwide, non-exclusive, no-charge, royalty-free, irrevocable
      copyright license to reproduce, prepare Derivative Works of,
      publicly display, publicly perform, sublicense, and distribute the
      Work and such Derivative Works in Source or Object form.

   3. Grant of Patent License. Subject to the terms and conditions of
      this License, each Contributor hereby grants to You a perpetual,
      worldwide, non-exclusive, no-charge, royalty-free, irrevocable
      (except as stated in this section) patent license to make, have made,
      use, offer to sell, sell, import, and otherwise transfer the Work,
      where such license applies only to those patent claims licensable
      by such Contributor that are necessarily infringed by their
      Contribution(s) alone or by combination of their Contribution(s)
      with the Work to which such Contribution(s) was submitted. If You
      institute patent litigation against any entity (including a
      cross-claim or counterclaim in a lawsuit) alleging that the Work
      or a Contribution incorporated within the Work constitutes direct
      or contributory patent infringement, then any patent licenses
      granted to You under this License for that Work shall terminate
      as of the date such litigation is filed.

   4. Redistribution. You may reproduce and distribute copies of the
      Work or Derivative Works thereof in any medium, with or without
      modifications, and in Source or Object form, provided that You
      meet the following conditions:

      (a) You must give any other recipients of the Work or
          Derivative Works a copy of this License; and

      (b) You must cause any modified files to carry prominent notices
          stating that You changed the files; and

      (c) You must retain, in the Source form of any Derivative Works
          that You distribute, all copyright, patent, trademark, and
          attribution notices from the Source form of the Work,
          excluding those notices that do not pertain to any part of
          the Derivative Works; and

      (d) If the Work includes a "NOTICE" text file as part of its
          distribution, then any Derivative Works that You distribute must
          include a readable copy of the attribution notices contained
          within such NOTICE file, excluding those notices that do not
          pertain to any part of the Derivative Works, in at least one
          of the following places: within a NOTICE text file distributed
          as part of the Derivative Works; within the Source form or
          documentation, if provided along with the Derivative Works; or,
          within a display generated by the Derivative Works, if and
          wherever such third-party notices normally appear. The contents
          of the NOTICE file are for informational purposes only and
          do not modify the License. You may add Your own attribution
          notices within Derivative Works that You distribute, alongside
          or as an addendum to the NOTICE text from the Work, provided
          that such additional attribution notices cannot be construed
          as modifying the License.

      You may add Your own copyright statement to Your modifications and
      may provide additional or different license terms and conditions
      for use, reproduction, or distribution of Your modifications, or
      for any such Derivative Works as a whole, provided Your use,
      reproduction, and distribution of the Work otherwise complies with
      the conditions stated in this License.

   5. Submission of Contributions. Unless You explicitly state otherwise,
      any Contribution intentionally submitted for inclusion in the Work
      by You to the Licensor shall be under the terms and conditions of
      this License, without any additional terms or conditions.
      Notwithstanding the above, nothing herein shall supersede or modify
      the terms of any separate license agreement you may have executed
      with Licensor regarding such Contributions.

   6. Trademarks. This License does not grant permission to use the trade
      names, trademarks, service marks, or product names of the Licensor,
      except as required for reasonable and customary use in describing the
      origin of the Work and reproducing the content of the NOTICE file.

   7. Disclaimer of Warranty. Unless required by applicable law or
      agreed to in writing, Licensor provides the Work (and each
      Contributor provides its Contributions) on an "AS IS" BASIS,
      WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or
      implied, including, without limitation, any warranties or conditions
      of TITLE, NON-INFRINGEMENT, MERCHANTABILITY, or FITNESS FOR A
      PARTICULAR PURPOSE. You are solely responsible for determining the
      appropriateness of using or redistributing the Work and assume any
      risks associated with Your exercise of permissions under this License.

   8. Limitation of Liability. In no event and under no legal theory,
      whether in tort (including negligence), contract, or otherwise,
      unless required by applicable law (such as deliberate and grossly
      negligent acts) or agreed to in writing, shall any Contributor be
      liable to You for damages, including any direct, indirect, special,
      incidental, or consequential damages of any character arising as a
      result of this License or out of the use or inability to use the
      Work (including but not limited to damages for loss of goodwill,
      work stoppage, computer failure or malfunction, or any and all
      other commercial damages or losses), even if such Contributor
      has been advised of the possibility of such damages.

   9. Accepting Warranty or Additional Liability. While redistributing
      the Work or Derivative Works thereof, You may choose to offer,
      and charge a fee for, acceptance of support, warranty, indemnity,
      or other liability obligations and/or rights consistent with this
      License. However, in accepting such obligations, You may act only
      on Your own behalf and on Your sole responsibility, not on behalf
      of any other Contributor, and only if You agree to indemnify,
      defend, and hold each Contributor harmless for any liability
      incurred by, or claims asserted against, such Contributor by reason
      of your accepting any such warranty or additional liability.

   END OF TERMS AND CONDITIONS

   APPENDIX: How to apply the Apache License to your work.

      To apply the Apache License to your work, attach the following
      boilerplate notice, with the fields enclosed by brackets "[]"
      replaced with your own identifying information. (Don't include
      the brackets!)  The text should be enclosed in the appropriate
      comment syntax for the file format. We also recommend that a
      file or class name and description of purpose be included on the
      same "printed page" as the copyright notice for easier
      identification within third-party archives.

   Copyright [yyyy] [name of copyright owner]

   Licensed under the Apache License, Version 2.0 (the "License");
   you may not use this file except in compliance with the License.
   You may obtain a copy of the License at

       http://www.apache.org/licenses/LICENSE-2.0

   Unless required by applicable law or agreed to in writing, software
   distributed under the License is distributed on an "AS IS" BASIS,
   WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
   See the License for the specific language governing permissions and
   limitations under the License.

## Source Sans Pro embedded in SVG documentation

Copyright 2010 - 2018 Adobe Systems Incorporated (http://www.adobe.com/),
with Reserved Font Name 'Source'.

The checked-in D2 SVG diagrams embed subsets of Source Sans Pro. The font
metadata identifies SIL Open Font License 1.1; this applies to the embedded
font software, not to the diagrams as a whole.

SIL OPEN FONT LICENSE Version 1.1 - 26 February 2007
-----------------------------------------------------------

PREAMBLE
The goals of the Open Font License (OFL) are to stimulate worldwide
development of collaborative font projects, to support the font creation
efforts of academic and linguistic communities, and to provide a free and
open framework in which fonts may be shared and improved in partnership
with others.

The OFL allows the licensed fonts to be used, studied, modified and
redistributed freely as long as they are not sold by themselves. The
fonts, including any derivative works, can be bundled, embedded,
redistributed and/or sold with any software provided that any reserved
names are not used by derivative works. The fonts and derivatives,
however, cannot be released under any other type of license. The
requirement for fonts to remain under this license does not apply
to any document created using the fonts or their derivatives.

DEFINITIONS
"Font Software" refers to the set of files released by the Copyright
Holder(s) under this license and clearly marked as such. This may
include source files, build scripts and documentation.

"Reserved Font Name" refers to any names specified as such after the
copyright statement(s).

"Original Version" refers to the collection of Font Software components as
distributed by the Copyright Holder(s).

"Modified Version" refers to any derivative made by adding to, deleting,
or substituting -- in part or in whole -- any of the components of the
Original Version, by changing formats or by porting the Font Software to a
new environment.

"Author" refers to any designer, engineer, programmer, technical
writer or other person who contributed to the Font Software.

PERMISSION & CONDITIONS
Permission is hereby granted, free of charge, to any person obtaining
a copy of the Font Software, to use, study, copy, merge, embed, modify,
redistribute, and sell modified and unmodified copies of the Font
Software, subject to the following conditions:

1) Neither the Font Software nor any of its individual components,
in Original or Modified Versions, may be sold by itself.

2) Original or Modified Versions of the Font Software may be bundled,
redistributed and/or sold with any software, provided that each copy
contains the above copyright notice and this license. These can be
included either as stand-alone text files, human-readable headers or
in the appropriate machine-readable metadata fields within text or
binary files as long as those fields can be easily viewed by the user.

3) No Modified Version of the Font Software may use the Reserved Font
Name(s) unless explicit written permission is granted by the corresponding
Copyright Holder. This restriction only applies to the primary font name as
presented to the users.

4) The name(s) of the Copyright Holder(s) or the Author(s) of the Font
Software shall not be used to promote, endorse or advertise any
Modified Version, except to acknowledge the contribution(s) of the
Copyright Holder(s) and the Author(s) or with their explicit written
permission.

5) The Font Software, modified or unmodified, in part or in whole,
must be distributed entirely under this license, and must not be
distributed under any other license. The requirement for fonts to
remain under this license does not apply to any document created
using the Font Software.

TERMINATION
This license becomes null and void if any of the above conditions are
not met.

DISCLAIMER
THE FONT SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND,
EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO ANY WARRANTIES OF
MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT
OF COPYRIGHT, PATENT, TRADEMARK, OR OTHER RIGHT. IN NO EVENT SHALL THE
COPYRIGHT HOLDER BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY,
INCLUDING ANY GENERAL, SPECIAL, INDIRECT, INCIDENTAL, OR CONSEQUENTIAL
DAMAGES, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING
FROM, OUT OF THE USE OR INABILITY TO USE THE FONT SOFTWARE OR FROM
OTHER DEALINGS IN THE FONT SOFTWARE.

## Goldmark

Native Markdown text primitives derive from github.com/yuin/goldmark v1.8.5,
pinned by D2 v0.9.0. Rust translations retain the following MIT terms.
Translated source files are marked `MPL-2.0 AND MIT`: Nelson Chen's Rust
translation and modifications are MPL-2.0, while the corresponding Goldmark
material remains available under its original MIT license.

MIT License

Copyright (c) 2019 Yusuke Inuzuka

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.

Noto Color Emoji COLRv1 2.051: pinned unmodified compressed asset from original Go D2 v0.9.0; SIL Open Font License 1.1. See crates/d2-rs-fontface/assets/NotoColorEmoji-OFL.txt and its README.md for provenance.

Gensymb configuration, upright font parsing and SVG unknown-text
metrics/rendering are translated from pinned mathjax-go v0.1.0
(MathJax Consortium, Copyright 2017–2022, Apache-2.0). The generated degree
path retains the existing MathJax font-data notice and license.

The generated-math scene checks and micro outline are translated
from original D2 v0.9.0 d2renderers/d2svgimport/mathjax_text.go and the
scene-builder math asset rules, under the original D2 MPL-2.0 terms. The micro
scene outline is independently authored original D2 geometry, not a font path.

TeXAtom local placement/bounds from `internal/svg/texatom.go` and
`wrapper.go`, embellished character script-base behavior and the math-hyphen
entry in `internal/tex/source_tables_base_maps.go` are translated from pinned
mathjax-go v0.1.0. Copyright 2017–2022 The MathJax Consortium; Apache-2.0. The
implementation retains the source attribution.

CD parsing/layout/delimiter assembly, dimension conversion, font
scope/identifier handling and recoverable error metrics are translated from
pinned mathjax-go v0.1.0 (Copyright 2017–2022 The MathJax Consortium,
Apache-2.0). The calligraphic/italic generated tables and original
U+23D0/U+2016 glyph paths retain the existing MathJax font-data license.
The generated-error font-family style diagnostic follows original D2
v0.9.0 d2svgimport/style.go (MPL-2.0).

Deferred delimiter paint, script/local label frames, style and
negative-depth handling translate pinned mathjax-go v0.1.0
internal/svg/{mo,mtable,msubsup,mpadded}.go and tex/environments.go.
Copyright 2017–2022 The MathJax Consortium; Apache-2.0. The original D2
MPL-2.0 and font notices remain intact.

The original mathjax-go v0.1.0 internal/tex/environments.go
splitTable/captureEnvironment/parseCDRow and parser.go
readControlSequence/readArgument are translated: immediate optional row
annotations, matching nested environment capture and earliest recoverable
errors. Original MathJax Consortium Apache-2.0 attribution is retained.

The original mathjax-go v0.1.0 tex/parser.go numeric mn runs,
svg/wrapper.go token IC/unknown-font/isCJK metrics and svg/msubsup.go numeric
script core semantics are translated. MathJax Consortium Apache-2.0
attribution is retained. `crates/d2-rs-math/src/number_unicode.rs` derives
Go 1.27.0 Unicode 17 IsDigit ranges; Go Authors BSD-3-Clause notices above
apply.

The original mathjax-go v0.1.0 layout/lengths.go
relative/absolute/size-dependent dimensions, tex/commands.go horizontal
spacing, tex/environments.go raw CD minimum-label kern, svg/wrapper.go
mspace/signed bbox Append and svg/typesetter.go final viewport clamps are
translated. MathJax Consortium Apache-2.0 attribution is retained. Native
Unicode classification adds Go 1.27.0 digit/letter-union ranges; existing Go
Authors BSD-3-Clause notices apply.

The mhchem texifier is a native translation of pinned mathjax-go v0.1.0
internal/mhchem/texify.go (mhchemparser 4.1.1). Copyright 2015-2021 Martin
Hensel, 2026 luo-studio and 2026 erweixin; Apache-2.0 AND MIT. The MIT notice
is retained in crates/d2-rs-math/licenses/mhchem-go-tex-MIT.txt. Original
PROVENANCE.md maps the pinned parser and initial Go translation.

Pinned mathjax-go v0.1.0 internal/mhchem/jsregexp.go and patterns.go are
translated into native Rust. Its exact data/patterns.json is retained;
eleven inline patterns are extracted unchanged from original patterns.go.
Copyright 2015-2021 Martin Hensel, 2026 luo-studio and 2026 erweixin;
Apache-2.0 AND MIT; the existing mhchem-go-tex-MIT.txt notice applies.

The pinned mathjax-go v0.1.0 internal/mhchem state-machine driver, nullable
buffers and all action handlers are translated into Rust. data/machines.json
is copied exactly (14 machines, 51 states, 1277 transitions).
Copyright 2015-2021 Martin Hensel, 2026 luo-studio and 2026 erweixin;
Apache-2.0 AND MIT; existing mhchem-go-tex-MIT.txt is retained.

Pinned mathjax-go v0.1.0 tex/commands.go phantom, smash and lap commands,
parser.go argument/bracket reading, svg/mpadded.go, mphantom.go and
transparent script-core H/D from msubsup.go are translated. Root bounds
retain msqrt.go em arithmetic before scaling when smash exposes isolated D.
Copyright 2017-2022 The MathJax Consortium; Apache-2.0 notices apply.

Default mtext glyph advances and local frames are additionally translated
from original mathjax-go svg/wrapper.go: no mi italic correction is applied.

mathjax-go v0.1.0 tex/commands.go mathClass, parser.go
groups/arguments/scripts, tex/inheritance.go and svg/texclass.go, texatom.go,
msubsup.go, munderover.go, mo.go and wrapper.go are translated into native
`crates/d2-rs-math/src/classes.rs` and `lib.rs`. Source normal-font long
arrows and the reachable SmallOp/-tex-variant font subsets are generated from
data_normal_gen.go, data_smallop_gen.go and data_tex_variant_gen.go.
Copyright 2017-2022/2018-2022 The MathJax Consortium; Apache-2.0 notices and
original font attribution apply.

`crates/d2-rs-math/src/source_symbols.rs` is generated from pinned
mathjax-go v0.1.0 `internal/tex/source_tables_*_maps.go` (MathJax 3.2.2
SymbolMap data); the regenerated `src/tex_variant_glyphs.rs`,
`src/large_glyphs.rs` and `src/smallop_glyphs.rs` are complete copies of the
corresponding mathjax-go `internal/font/data_*_gen.go` tables (MathJax 3.2.2
TeX font data). The added parser/layout translations (tex/commands.go,
tex/physics_handlers.go, tex/inheritance.go, svg/msqrt.go, svg/mo.go,
svg/stretch.go, svg/mpadded.go, svg/texclass.go, svg/wrapper.go) are Go-to-Rust
translations and modifications of mathjax-go, itself a translation of MathJax
3.2.2: Copyright (c) 2009-2022 The MathJax Consortium, Apache License 2.0
(see mathjax-go LICENSE and NOTICE, whose MathJax, Omar Al-Ithawi, mhchemParser
and go-tex attributions are retained).

The pinned module's complete license and attribution texts are distributed in
`crates/d2-rs-math/licenses/mathjax-go-LICENSE.txt`,
`crates/d2-rs-math/licenses/mathjax-go-NOTICE.txt`, and
`crates/d2-rs-math/licenses/mathjax-go-MIT-Luo-Studio.txt` (copied without
changes).

## D2 command logging

The native CLI stderr line decoration follows `cmdlog.prefixWriter` in
`github.com/d2lang/util-go v0.2.0`, the dependency pinned by D2 v0.9.0.
This source reference retains its ISC license:

```text
Copyright (c) 2022 Terrastruct Inc. info@terrastruct.com

Permission to use, copy, modify, and distribute this software for any
purpose with or without fee is hereby granted, provided that the above
copyright notice and this permission notice appear in all copies.

THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHOR DISCLAIMS ALL WARRANTIES
WITH REGARD TO THIS SOFTWARE INCLUDING ALL IMPLIED WARRANTIES OF
MERCHANTABILITY AND FITNESS. IN NO EVENT SHALL THE AUTHOR BE LIABLE FOR
ANY SPECIAL, DIRECT, INDIRECT, OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES
WHATSOEVER RESULTING FROM LOSS OF USE, DATA OR PROFITS, WHETHER IN AN
ACTION OF CONTRACT, NEGLIGENCE OR OTHER TORTIOUS ACTION, ARISING OUT OF
OR IN CONNECTION WITH THE USE OR PERFORMANCE OF THIS SOFTWARE.
```

Noto Color Emoji COLRv1 2.051: pinned unmodified compressed asset from original Go D2 v0.9.0; SIL Open Font License 1.1. See crates/d2-rs-fontface/assets/NotoColorEmoji-OFL.txt and its README.md for provenance.

## Chroma code syntax and themes

The native Go and Go-template token rules, token splitting, and style inheritance
in `crates/d2-rs-text/src/code.rs` are translated from Chroma v2.27.0, the
dependency pinned by original Go D2 v0.9.0. Its `github.xml` and
`catppuccin-mocha.xml` code styles are preserved byte-for-byte in
`crates/d2-rs-text/assets/chroma/`. These portions retain Chroma's MIT terms:
Copyright (C) 2017 Alec Thomas. The complete original notice is preserved at
`crates/d2-rs-text/assets/chroma/COPYING`; its bundled font notice is retained
there as part of the original file, although no Chroma font bytes are imported.
Rust translation and modifications: Copyright 2026 Nelson Chen.

The pinned regexp2 v2.6.0 category behavior used by the translated rules is
identified by source comparison; its original MIT notice is preserved at
`crates/d2-rs-text/assets/chroma/COPYING-regexp2`. Native matching executes
through the Rust regex crate. Native Markdown/YAML and JavaScript/JSON use pinned
original `lexers/markdown.go`, `embedded/{yaml,javascript,json}.xml` assets and
source RegexLexer/Include/Push/Pop/UsingSelf/UsingByGroup execution. Native registry configuration and lookup follow original Chroma `registry.go`
and `lexer.go`; the pinned definition inventory is retained in
`crates/d2-rs-text/assets/chroma/registry-config.json`. Regex features
use the pure Rust fancy-regex crate.

Native `crates/d2-rs-text/src/uniseg.rs` and generated `uniseg_data.rs` translate pinned github.com/rivo/uniseg v0.4.7 line/grapheme/width rules (MIT, Copyright (c) 2019 Oliver Kuederle) and Unicode 15.0.0 property payloads. Complete MIT and Unicode copyright/permission texts are retained in crates/d2-rs-text/licenses/UNISEG-LICENSE.txt and UNICODE-LICENSE.txt. Go 1.27 Unicode 17 symbol/mark categories retain the Go Authors 2009 BSD3 notice and complete GO-LICENSE.txt in the same directory. Native `crates/d2-rs-text/src/measure.rs` CSS fallback follows original D2 v0.9.0 textmeasure algorithms under MPL-2.0. The ordered tables are generated by scripts/generate-uniseg-boundaries.py from the pinned Go unicode/tables.go.

`crates/d2-rs-render/src/image_mime_registry.rs` extends the
native MIME fallback with Go 1.27.0 `mime/type.go`, `type_unix.go`, `grammar.go`
and reachable `mediatype.go` registration and parameter serialization behavior.
Copyright 2010 The Go Authors. All rights reserved. Go BSD-3-Clause terms above
apply. Unix host databases are read in the original source order; the native
algorithms do not use a Go process.

Chroma v2.27.0's 279 `lexers/embedded/*.xml` definitions are preserved
byte-for-byte in `crates/d2-rs-text/assets/chroma/lexers/` and executed by
`src/code/xml.rs`, which translates `regexp.go`, `mutators.go` and `emitters.go`.
`lisp-mappings.json` retains `lexers/cl.go` and `lexers/emacs.go` word tables;
`samples.json` includes source excerpts from `lexers/testdata/*.actual` (each
sample names its source) and handwritten seeds. The native engine also ports
`delegate.go`, `coalesce.go` and `remap.go`. The same Chroma MIT terms,
Copyright (C) 2017 Alec Thomas, apply.
`assets/chroma/custom/*.xml` serializes the pinned declarative rule constructors
in `lexers/{caddyfile,gemtext,genshi,haxe,http,markless,raku,rst,svelte}.go` as source
inputs. `scripts/vendor-chroma-lexers.py` reproduces these definitions and the
registry MIME configuration in `assets/chroma/mime-types.json`. Native HTTP
content-type delegation and reStructuredText code-block emitters in
`src/code/xml.rs` translate `lexers/http.go` and `lexers/rst.go`, under the
same Chroma MIT terms and copyright.
`src/code/haxe.rs` translates `lexers/haxe.go`'s preprocessor mutator under
the same terms. Its shared stack capacity calculation follows Go 1.27
`runtime/slice.go`, `runtime/msize.go` and `internal/runtime/gc/sizeclasses.go`,
under the Go Authors' BSD terms reproduced above.
`src/code/raku.rs` translates `lexers/raku.go`'s runtime rule replacement,
delimiter matching, quote and POD emitters. `assets/chroma/raku-brackets.json`
retains its Unicode delimiter table under the same Chroma MIT terms.
`src/code/chroma_style.rs` translates Chroma's `types.go` TokenType values,
`style.go` entry parsing/inheritance/synthesis, `colour.go` colour handling and
`formatters/svg/svg.go` StyleEntryToSVG. Same MIT terms and copyright as above.

## csscolorparser v0.1.8 acceptance rules

Gradient-stop syntax validation in `crates/d2-rs-language/src/compiler/style_colors.rs`
translates reachable acceptance rules from the pinned original Go dependency
`github.com/mazznoer/csscolorparser v0.1.8/colorparser.go`. Color conversion is
not translated or claimed by this validator.

MIT License

Copyright (c) 2020 Nor Khasyatillah

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.

## Go encoding/xml name ranges

The XML-name ranges in crates/d2-rs-math/src/image_svg/go_xml_names.rs are translated
from the Go 1.27.0 toolchain used by the pinned D2 source. They retain the
Go Authors BSD license:

Copyright 2009 The Go Authors.

Redistribution and use in source and binary forms, with or without
modification, are permitted provided that the following conditions are
met:

   * Redistributions of source code must retain the above copyright
notice, this list of conditions and the following disclaimer.
   * Redistributions in binary form must reproduce the above
copyright notice, this list of conditions and the following disclaimer
in the documentation and/or other materials provided with the
distribution.
   * Neither the name of Google LLC nor the names of its
contributors may be used to endorse or promote products derived from
this software without specific prior written permission.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS
"AS IS" AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT
LIMITED TO, THE IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR
A PARTICULAR PURPOSE ARE DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT
OWNER OR CONTRIBUTORS BE LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL,
SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT
LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES; LOSS OF USE,
DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON ANY
THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT
(INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE
OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.

The BSD-3-Clause Go encoding/xml notice above also covers the translated
procInst lookup in crates/d2-rs-math/src/image_svg/svg_import_envelope.rs:decoder_field
(Go 1.27.0). The other envelope admission rules derive from D2 v0.9.0 MPL-2.0
import.go and lib/svg/doctype.go.

The Go encoding/xml BSD-3-Clause notice also covers reachable lexical, entity,
character-range and element-stack translation in
crates/d2-rs-math/src/image_svg/svg_asset_xml.rs (Go 1.27.0). D2 asset root/admission
rules retain MPL-2.0 mapping to lib/imageasset/probe.go.

The XML name character ranges in crates/d2-rs-math/src/image_svg/go_xml_names.rs are translated from
Go 1.27.0 encoding/xml, used by the pinned D2 reference toolchain.
https://go.dev/src/encoding/xml/xml.go

## Brotli reader error compatibility

Native SVG codec reader boundary/error compatibility maps to github.com/andybalholm/brotli v1.2.2 reader.go and decode.go, used by pinned original D2 v0.9.0. The decoder itself executes the native Rust brotli-decompressor crate.

Copyright (c) 2009, 2010, 2013-2016 by the Brotli Authors.

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in
all copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT.  IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN
THE SOFTWARE.

Nested viewport/aspect/clip transport and grouped clip painting
translate original D2 v0.9.0 d2svgimport, d2scene/aspect.go and d2raster
source under MPL-2.0. Existing MathJax-generated glyph licenses and source
notices remain unchanged.

## golang.org/x/net HTML name folding

The native Markdown HTML name adapter in `crates/d2-rs-text/src/markdown/html_names.rs` follows
`golang.org/x/net v0.57.0/html/token.go:Tokenizer.TagName/TagAttr`.
Copyright 2010 The Go Authors. All rights reserved.

Copyright 2009 The Go Authors.

Redistribution and use in source and binary forms, with or without
modification, are permitted provided that the following conditions are
met:

   * Redistributions of source code must retain the above copyright
notice, this list of conditions and the following disclaimer.
   * Redistributions in binary form must reproduce the above
copyright notice, this list of conditions and the following disclaimer
in the documentation and/or other materials provided with the
distribution.
   * Neither the name of Google LLC nor the names of its
contributors may be used to endorse or promote products derived from
this software without specific prior written permission.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS
"AS IS" AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT
LIMITED TO, THE IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR
A PARTICULAR PURPOSE ARE DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT
OWNER OR CONTRIBUTORS BE LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL,
SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT
LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES; LOSS OF USE,
DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON ANY
THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT
(INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE
OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.

The Go standard-library BSD3 notice above also covers native `crates/d2-rs-text/src/xml.rs` lexical, stack and error-message validation and `crates/d2-rs-text/src/go_xml_names.rs` XML-name ranges from pinned Go 1.27 encoding/xml/xml.go. `crates/d2-rs-text/src/go_quote.rs` reuses the repository's strconv.Quote translation and Unicode 17 printability generated from the same Go toolchain. Original Go Authors2009 copyright and source notices remain in these files. The raw compiler HTML projection corrections follow pinned Goldmark v1.8.5 renderer/html/html.go, util/util.go and extension/table.go under its retained MIT notice.

Local shaping dependency: vendor/rustybuzz-go-text contains Rustybuzz 0.20.1 under its retained MIT LICENSE. D2-specific lookup/digest changes follow pinned go-text/typesetting v0.3.4 HarfBuzz-derived code (Unlicense OR BSD-3-Clause; GO_TEXT_LICENSE retained); source mapping and modifications are recorded in vendor/rustybuzz-go-text/SOURCE.md. Corresponding source is included in this repository.

Unicode lookup tables and source control data follow pinned go-text/typesetting v0.3.4 internal/unicodedata (Unlicense OR BSD-3-Clause), with its GO_TEXT_LICENSE retained in vendor/rustybuzz-go-text. Modified HarfBuzz portions additionally retain the pinned Go harfbuzz MIT copyright/license in GO_HARFBUZZ_LICENSE. Local package metadata describes both source notices. The tables are generated by scripts/generate-source-unicode.py.

CD parsing/layout/delimiter assembly, dimension conversion, font
scope/identifier handling and recoverable error metrics are translated from
pinned mathjax-go v0.1.0 (Copyright 2017–2022 The MathJax Consortium,
Apache-2.0). The calligraphic/italic generated tables and original
U+23D0/U+2016 glyph paths retain the existing MathJax font-data license.
The generated-error font-family style diagnostic follows original D2
v0.9.0 d2svgimport/style.go (MPL-2.0).

Nested viewport/aspect/clip transport and grouped clip painting
translate original D2 v0.9.0 d2svgimport, d2scene/aspect.go and d2raster
source under MPL-2.0. Existing MathJax-generated glyph licenses and source
notices remain unchanged.

Deferred delimiter paint, script/local label frames, style and
negative-depth handling translate pinned mathjax-go v0.1.0
internal/svg/{mo,mtable,msubsup,mpadded}.go and tex/environments.go.
Copyright 2017–2022 The MathJax Consortium; Apache-2.0. The original D2
MPL-2.0 and font notices remain intact.

The original mathjax-go v0.1.0 internal/tex/environments.go
splitTable/captureEnvironment/parseCDRow and parser.go
readControlSequence/readArgument are translated: immediate optional row
annotations, matching nested environment capture and earliest recoverable
errors. Original MathJax Consortium Apache-2.0 attribution is retained.

The original mathjax-go v0.1.0 tex/parser.go numeric mn runs,
svg/wrapper.go token IC/unknown-font/isCJK metrics and svg/msubsup.go numeric
script core semantics are translated. MathJax Consortium Apache-2.0
attribution is retained. `crates/d2-rs-math/src/number_unicode.rs` derives
Go 1.27.0 Unicode 17 IsDigit ranges; Go Authors BSD-3-Clause notices above
apply.

The original mathjax-go v0.1.0 layout/lengths.go
relative/absolute/size-dependent dimensions, tex/commands.go horizontal
spacing, tex/environments.go raw CD minimum-label kern, svg/wrapper.go
mspace/signed bbox Append and svg/typesetter.go final viewport clamps are
translated. MathJax Consortium Apache-2.0 attribution is retained. Native
Unicode classification adds Go 1.27.0 digit/letter-union ranges; existing Go
Authors BSD-3-Clause notices apply.

The mhchem texifier is a native translation of pinned mathjax-go v0.1.0
internal/mhchem/texify.go (mhchemparser 4.1.1). Copyright 2015-2021 Martin
Hensel, 2026 luo-studio and 2026 erweixin; Apache-2.0 AND MIT. The MIT notice
is retained in crates/d2-rs-math/licenses/mhchem-go-tex-MIT.txt. Original
PROVENANCE.md maps the pinned parser and initial Go translation.

Pinned mathjax-go v0.1.0 internal/mhchem/jsregexp.go and patterns.go are
translated into native Rust. Its exact data/patterns.json is retained;
eleven inline patterns are extracted unchanged from original patterns.go.
Copyright 2015-2021 Martin Hensel, 2026 luo-studio and 2026 erweixin;
Apache-2.0 AND MIT; the existing mhchem-go-tex-MIT.txt notice applies.

The pinned mathjax-go v0.1.0 internal/mhchem state-machine driver, nullable
buffers and all action handlers are translated into Rust. data/machines.json
is copied exactly (14 machines, 51 states, 1277 transitions).
Copyright 2015-2021 Martin Hensel, 2026 luo-studio and 2026 erweixin;
Apache-2.0 AND MIT; existing mhchem-go-tex-MIT.txt is retained.

Pinned mathjax-go v0.1.0 tex/commands.go phantom, smash and lap commands,
parser.go argument/bracket reading, svg/mpadded.go, mphantom.go and
transparent script-core H/D from msubsup.go are translated. Root bounds
retain msqrt.go em arithmetic before scaling when smash exposes isolated D.
Copyright 2017-2022 The MathJax Consortium; Apache-2.0 notices apply.

Default mtext glyph advances and local frames are additionally translated
from original mathjax-go svg/wrapper.go: no mi italic correction is applied.

mathjax-go v0.1.0 tex/commands.go mathClass, parser.go
groups/arguments/scripts, tex/inheritance.go and svg/texclass.go, texatom.go,
msubsup.go, munderover.go, mo.go and wrapper.go are translated into native
`crates/d2-rs-math/src/classes.rs` and `lib.rs`. Source normal-font long
arrows and the reachable SmallOp/-tex-variant font subsets are generated from
data_normal_gen.go, data_smallop_gen.go and data_tex_variant_gen.go.
Copyright 2017-2022/2018-2022 The MathJax Consortium; Apache-2.0 notices and
original font attribution apply.

Native `crates/d2-rs-text/src/uniseg.rs` and generated `uniseg_data.rs` translate pinned github.com/rivo/uniseg v0.4.7 line/grapheme/width rules (MIT, Copyright (c) 2019 Oliver Kuederle) and Unicode 15.0.0 property payloads. Complete MIT and Unicode copyright/permission texts are retained in crates/d2-rs-text/licenses/UNISEG-LICENSE.txt and UNICODE-LICENSE.txt. Go 1.27 Unicode 17 symbol/mark categories retain the Go Authors 2009 BSD3 notice and complete GO-LICENSE.txt in the same directory. Native `crates/d2-rs-text/src/measure.rs` CSS fallback follows original D2 v0.9.0 textmeasure algorithms under MPL-2.0. The ordered tables are generated by scripts/generate-uniseg-boundaries.py from the pinned Go unicode/tables.go.

`crates/d2-rs-render/src/image_mime.rs` and
`image_mime_data.rs` translate the ordered content-type sniffer and built-in
MIME extension table from pinned Go 1.27.0 `net/http/internal/sniff.go` and
`mime/type.go`. Copyright 2010, 2011 The Go Authors. All rights reserved.
These portions retain the Go BSD-3-Clause license reproduced above. Regenerate
the tables with `scripts/generate-http-mime-tables.py`. System MIME database
registrations are not included in this built-in translation.

`crates/d2-rs-render/src/image_mime_registry.rs` extends the
native MIME fallback with Go 1.27.0 `mime/type.go`, `type_unix.go`, `grammar.go`
and reachable `mediatype.go` registration and parameter serialization behavior.
Copyright 2010 The Go Authors. All rights reserved. Go BSD-3-Clause terms above
apply. Unix host databases are read in the original source order; the native
algorithms do not use a Go process.

The PDF-specific cmap length mode in `crates/d2-rs-text/src/subset_font.rs`
follows `codeberg.org/go-pdf/fpdf v0.12.0/utf8fontfile.go`; the full upstream
MIT notice is retained under "Go-PDF / FPDF document and UTF-8 font
serialization" below.

## Go reduced SVG transform trigonometry

`crates/d2-rs-math/src/svg_trig.rs` derives reduced-input Sincos/Tan from
Go 1.27.0 math/sincos.go, sin.go and tan.go. Copyright 2010 The Go Authors.
The Go BSD license reproduced above applies.

Cephes Math Library Release 2.8: June, 2000
Copyright 1984, 1987, 1989, 1992, 2000 by Stephen L. Moshier

The readme file at http://netlib.sandia.gov/cephes/ says:
Some software in this archive may be from the book _Methods and
Programs for Mathematical Functions_ (Prentice-Hall or Simon & Schuster
International, 1989) or from the Cephes Mathematical Library, a
commercial product. In either event, it is copyrighted by the author.
What you see here may be used freely but it comes with no support or
guarantee.

The two known misprints in the book are repaired here in the
source listings for the gamma function and the incomplete beta
integral.

Stephen L. Moshier
moshier@na-net.ornl.gov

## Native pinned CSS color parser

`crates/d2-rs-math/src/svg_color.rs` and `css_named_colors.json` derive from
csscolorparser v0.1.8, Copyright (c) 2020 Nor Khasyatillah.

```text
MIT License

Copyright (c) 2020 Nor Khasyatillah

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

Native WebP configuration admission in `crates/d2-rs-math/src/svg_image_config.rs`
follows `golang.org/x/image` v0.45.0 `webp/decode.go`, `vp8/decode.go` and
`vp8l/decode.go` header admission. Copyright 2011 The Go Authors. All rights
reserved. The complete Go Authors BSD-3-Clause conditions and disclaimer above
apply to this translation.

Native `crates/d2-rs-core/src/jpeg_idct.rs` follows Go 1.27 `image/jpeg/dct.go`. Copyright 2025 The Go Authors. All rights reserved. The Go BSD-3-Clause conditions and disclaimer reproduced above apply.

Native `crates/d2-rs-core/src/jpeg_entropy.rs` follows Go 1.27 `image/jpeg/huffman.go` and buffered entropy reading in `image/jpeg/reader.go`. Copyright 2009 The Go Authors. All rights reserved. The Go BSD-3-Clause conditions and disclaimer reproduced above apply.

Native `crates/d2-rs-core/src/jpeg_decode.rs` follows Go 1.27 `image/jpeg/reader.go`, `scan.go`, and `image/color/ycbcr.go`, with the separately mapped inverse DCT and entropy reader. Copyright 2009, 2012, 2025 The Go Authors. All rights reserved. The Go BSD-3-Clause conditions and disclaimer reproduced above apply.

`crates/d2-rs-core/src/jpeg_first.rs` follows D2 v0.9.0 rasterimage JPEG/contextReader behavior and the pinned Go 1.27 bufio reader. Go bufio portions: Copyright 2009 The Go Authors. All rights reserved. The Go BSD-3-Clause conditions and disclaimer reproduced above apply. Functional JPEG input cancellation preserves the source reader result semantics.

## ELK SIMPLE Box packing

`crates/d2-rs-elk/src/box_layout.rs` translates SIMPLE sorting and row packing
from `github.com/d2lang/elk-go v0.2.0/internal/box/provider.go`, pinned by D2
v0.9.0. Copyright (c) 2009, 2020 Kiel University and others; Copyright (c)
2015, 2020 Kiel University and others. These portions retain EPL-2.0. The
complete license is reproduced in `crates/d2-rs-elk/ELK-EPL-2.0.txt`
(`ELK-EPL-2.0.txt` within the packaged ELK crate). Rust translation and
modifications: Copyright 2026 Nelson Chen.

## ELK Fixed and utility-provider scope preparation

`crates/d2-rs-elk/src/fixed_layout.rs` and `utility.rs` translate reachable
parts of pinned `elk-go v0.2.0/internal/fixed/provider.go`,
`internal/common/elkutil/elkutil.go`, `internal/core/engine.go` and
`internal/geometry/geometry.go`. Copyright (c) 2010, 2019 and 2015, 2020
Kiel University and others (Fixed); Copyright (c) 2008, 2020 Kiel University
and others (ElkUtil); Copyright (c) 2008, 2018 Kiel University and others,
2018, 2020 TypeFox GmbH and others, and 2026 The elk-go Authors (engine);
Copyright (c) 2010, 2015; 2010, 2017; 2014, 2017; 2016 Kiel University and
others, and 2026 The elk-go Authors (geometry). These portions retain
EPL-2.0; complete terms are in `crates/d2-rs-elk/ELK-EPL-2.0.txt`
(`ELK-EPL-2.0.txt` in the packaged crate). Rust translation/modifications:
Copyright 2026 Nelson Chen.

## ELK default Rectpacking chain

`crates/d2-rs-elk/src/rectpacking/` translates greedy width estimation and
the default single-pass row/block compactor from pinned `elk-go v0.2.0`
`internal/rectpacking/provider.go` and `second_iteration.go`. Copyright (c)
2018, 2020 Kiel University and others; Copyright (c) 2020 Kiel University
and others; Copyright (c) 2026 The elk-go Authors. These portions retain
EPL-2.0; complete terms are in `crates/d2-rs-elk/ELK-EPL-2.0.txt`
(`ELK-EPL-2.0.txt` in the packaged crate). Rust translation/modifications:
Copyright 2026 Nelson Chen.

`rectpacking/micro.rs` also translates the pre/post-packing sizing and
label/port placement from pinned
`internal/common/nodespacing/node_label_and_size_calculator.go`: Copyright (c)
2014, 2015 Kiel University and others; Copyright (c) 2017, 2019 Kiel University
and others, under EPL-2.0. The complete terms above apply.

## ELK Random provider

`crates/d2-rs-elk/src/random_layout.rs` translates pinned elk-go v0.2.0
`internal/random/provider.go`: Copyright (c) 2010, 2015 Kiel University and
others; Copyright (c) 2015, 2020 Kiel University and others, under EPL-2.0.
The shared Java RNG remains the source-derived implementation in `layered.rs`.
Complete terms are in `crates/d2-rs-elk/ELK-EPL-2.0.txt` (`ELK-EPL-2.0.txt`
in the packaged crate). Rust translation/modifications: Copyright 2026
Nelson Chen.

## ELK Rectpacking option processors and option registry

The Rectpacking translation additionally covers the option-driven provider
processors, repeated row/block compaction and whitespace expansion in pinned
elk-go v0.2.0 `internal/rectpacking/{provider,second_iteration,options}.go`.
`utility.rs` and `src/fixtures/original_option_ids.txt` derive suffix resolution
and registered identifiers from `internal/core/registry.go` and the built-in
registrations, including `internal/builtins/frozen_options.go`. Copyright (c)
2012, 2015; 2015, 2020; 2016, 2020 Kiel University and others, and 2026 The
elk-go Authors; EPL-2.0. Existing Rectpacking notices also apply. Complete
terms: `crates/d2-rs-elk/ELK-EPL-2.0.txt` (packaged as `ELK-EPL-2.0.txt`).
Rust modifications: Copyright 2026 Nelson Chen.

## ELK Force and Stress providers

`crates/d2-rs-elk/src/force_layout/` translates elk-go v0.2.0
`internal/force/{graph,importer,components,provider,model,options}.go` and
`internal/stress/{provider,majorization,options}.go`: particle import, component
splitting/recombination, Fruchterman-Reingold and Eades force models, Java-shaped
priority-queue Dijkstra, and localized stress majorization. Copyright (c)
2008–2020 Kiel University and others; EPL-2.0. Native integration/tests:
Copyright 2026 Nelson Chen. Complete license: `crates/d2-rs-elk/ELK-EPL-2.0.txt`.

`utility.rs` also follows elk-go `internal/core/engine.go` receiver-local
algorithm resolution. `layered.rs` follows the sole-component transfer branch
of `internal/layered/components/simple_row_graph_placer.go`; existing Layered
notices continue to apply.

The Force Eades logarithm translates Go 1.27 `src/math/{log,frexp}.go`,
Copyright 2009 The Go Authors, under BSD-3-Clause (`GO-MATH-LICENSE.txt` in
the ELK crate). Its constants/algorithm retain the notice: Copyright(C)1993
Sun Microsystems, Inc. All rights reserved. Developed at SunPro, a Sun
Microsystems, Inc. business. Permission to use, copy, modify, and distribute
this software is freely granted, provided that this notice is preserved.
Arm64 fused evaluation follows the pinned compiler's disassembly.

## ELK Radial provider and numeric routines

`crates/d2-rs-elk/src/radial_layout/` translates elk-go v0.2.0
`internal/radial/{provider,options}.go`, including original wedge allocation,
ordering, overlap removal, compactors, optimization, rotation, sizing and routes.
Copyright (c) 2017 and 2017–2020 Kiel University and others; EPL-2.0. Its node
margin calculation follows `internal/common/nodespacing/node_margin_calculator.go`,
Copyright (c) 2010–2020 Kiel University and others. Complete ELK license:
`crates/d2-rs-elk/ELK-EPL-2.0.txt`.

`js_trig.rs` translates elk-go `internal/javacompat/jsmath.go`: fdlibm-derived
kernels, Copyright(C)1993 Sun Microsystems, Inc. All rights reserved. Developed
at SunSoft, a Sun Microsystems, Inc. business. Permission to use, copy, modify,
and distribute this software is freely granted, provided that this notice is
preserved. The original source was modified significantly by Google Inc.;
Copyright 2016 the V8 project authors, Copyright (c) 2026 The elk-go Authors.
V8 BSD-3-Clause license: `crates/d2-rs-elk/V8-BSD-3-Clause.txt`.

`go_trig.rs` and `go_math.rs` translate pinned Go 1.27
`src/math/{sin,trig_reduce,atan,atan2,asin,hypot}.go`: Copyright 2009,2010,2011,2018
The Go Authors; BSD-3-Clause (`crates/d2-rs-elk/GO-MATH-LICENSE.txt`). Cephes-derived
portions retain Stephen L.Moshier's1984,1987,1989,1992,2000 copyright and permission
notice in `crates/d2-rs-elk/GO-TRIG-NOTICE.txt`. Native integration and tests:
Copyright 2026 Nelson Chen.

## ELK Mr. Tree provider

`crates/d2-rs-elk/src/mrtree/` translates pinned elk-go v0.2.0
`internal/mrtree/{model,options,provider}.go`: source component identity,
cycle-edge removal from placement incidence, dummy roots, fan/constraint ordering,
Walker placement, direction transforms, scan-line compaction, cycle/multilevel
routing, source packing and wire section transfer.

Copyright (c) 2013, 2015, 2017, 2020 Kiel University and others.
These portions retain EPL-2.0; see `crates/d2-rs-elk/ELK-EPL-2.0.txt`. Rust additions and modifications:
Copyright 2026 Nelson Chen, MPL-2.0.

## ELK SPoRE providers and Go runtime random primitives

`crates/d2-rs-elk/src/spore/` translates pinned elk-go v0.2.0
`internal/spore/` providers, importer, rectangle geometry, Delaunay triangulation,
ordered spanning trees, scanline overlap detection and tree growth/compaction.
Copyright (c) 2017 Kiel University and others; Copyright (c) 2026 The elk-go
Authors. These portions retain EPL-2.0.

`spore/random.rs` translates Go 1.27 internal/chacha8rand state/block functions
and math/rand's Int63/Float64 conversion. Copyright 2023 The Go Authors.
BSD-3-Clause terms are in GO-MATH-LICENSE.txt; source attribution is in
GO-CHACHA8-NOTICE.txt. Native modifications: Copyright 2026 Nelson Chen, MPL-2.0.

## rough-go v0.2.0 / Rough.js

Polygon path generation and zigzag scan-line filling are translated from the pinned D2 dependency
`github.com/d2lang/rough-go v0.2.0`, generator.go, renderer.go,
filler_zigzag.go, filler_scan_line_hachure.go and geometry.go.
`crates/d2-rs-sketch/src/generator.rs` translates the same module's core.go,
generator.go, renderer.go, path.go, number.go, math.go, geometry.go and
filler*.go (Rough.js 4.6.6, hachure-fill 0.5.2 and path-data-parser 0.1.0
derived), used by the d2sketch SVG port in
`crates/d2-rs-render/src/d2svg/sketch.rs`.

MIT License

Copyright (c) 2019 Preet Shihn
Copyright (c) 2020 Preet Shihn
Copyright (c) 2023 Preet Shihn
Copyright (c) 2026 rough-go contributors

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.

## go-quantize median cut

`crates/d2-rs-export/src/gif_quantize.rs` translates palette bucket construction,
median partitioning and mode selection from `github.com/ericpauley/go-quantize`
revision `ae555eb2afa4` (D2 v0.9.0's pinned dependency). The D2 xgif glue retains
MPL-2.0 terms; these translated quantizer portions retain BSD-3-Clause:

```text
BSD 3-Clause License

Copyright (c) 2017, Eric Pauley
All rights reserved.

Redistribution and use in source and binary forms, with or without
modification, are permitted provided that the following conditions are met:

* Redistributions of source code must retain the above copyright notice, this
  list of conditions and the following disclaimer.

* Redistributions in binary form must reproduce the above copyright notice,
  this list of conditions and the following disclaimer in the documentation
  and/or other materials provided with the distribution.

* Neither the name of the copyright holder nor the names of its
  contributors may be used to endorse or promote products derived from
  this software without specific prior written permission.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS"
AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE
IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE
DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT HOLDER OR CONTRIBUTORS BE LIABLE
FOR ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL
DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR
SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER
CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY,
OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE
OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.
```

## Go-PDF / FPDF document and UTF-8 font serialization

`crates/d2-rs-export/src/pdf_font.rs` and `pdf_document.rs` translate the UTF-8
font writer, CID width grouping, page streams, IDs and document/resource writer
from `codeberg.org/go-pdf/fpdf v0.12.0`, pinned by D2
v0.9.0. These translated portions retain the upstream MIT license:

```text
MIT License

Copyright (c) 2021 The Go-PDF authors
Copyright (c) 2020 David Barnes
Copyright (c) 2017 Kurt Jung and contributors acknowledged in the documentation

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

The Go encoding/xml lexical/name/envelope and Brotli reader compatibility
translations live in `crates/d2-rs-math/src/image_svg/`. Their original SPDX
headers are kept in the source files, and their full BSD-3-Clause/MIT notices
are reproduced in this file.

## Board output Unicode keys

`crates/d2-rs-cli/src/board_output/unicode_fold.rs` is generated from `golang.org/x/text`
v0.41.0 `cases.Fold` and `unicode/norm` properties (Unicode 17.0.0), pinned
by D2 master 6a7b48378. Copyright 2009 The Go Authors. The Go BSD-3-Clause
license above applies. Native NFC uses unicode-normalization 0.1.25, also
Unicode 17.0.0; its complete notices are in the generated DEPENDENCY_NOTICES.md.

## Watch authorization parsing

`crates/d2-rs-cli/src/watch.rs` translates reachable cookie/query parsing and
redirect escaping from Go 1.27.0 `src/net/http/{cookie,server}.go` and
`src/net/url/{url,gen_encoding_table}.go`. Copyright 2009, 2025 The Go
Authors. All rights reserved. The complete Go BSD-3-Clause terms above apply.

## ELK network-simplex layering

`crates/d2-rs-elk/src/layered/network_simplex.rs` translates pinned elk-go
v0.2.0 `internal/common/networksimplex/{adapter,network_simplex,n_graph}.go`,
including component edge import order, subtree pruning and reattachment.
Copyright (c) 2016, 2017 Kiel University and others; Copyright (c) 2026 The elk-go
Authors. Source-derived portions retain EPL-2.0; complete terms are in
`crates/d2-rs-elk/ELK-EPL-2.0.txt`. Rust translation/modifications retain
MPL-2.0.

## ELK Coffman–Graham and GWT priority queue

`crates/d2-rs-elk/src/layered/coffman_graham.rs` translates the pinned elk-go v0.2.0
`internal/layered/p2layers/coffman_graham.go`, Copyright 2016, 2020 Kiel
University and others; Copyright 2026 The elk-go Authors, under EPL-2.0.
`crates/d2-rs-elk/src/layered/gwt_priority_queue.rs` translates its GWT 2.11 offer/poll heap,
Copyright 2008 Google Inc.; Copyright 2026 The elk-go Authors, under Apache-2.0.
The source notices remain in those files; the complete Apache-2.0 license
appears above and the upstream licenses are preserved in the pinned source.
Rust translation and modifications: Copyright 2026 Nelson Chen.

## ELK greedy port distribution

`crates/d2-rs-elk/src/layered/greedy_port_distributor.rs` translates elk-go v0.2.0
`internal/layered/p3order/greedy_port_distributor.go` and the adjacent-port
counter from `crossings_counter.go`. Copyright 2016 Kiel University and
2016, 2018 Kiel University and others; elk-go adaptations Copyright 2026 The
elk-go Authors. These portions retain EPL-2.0, whose terms are reproduced above.

## MathJax TeX font data and layout

`crates/d2-rs-math/src/glyphs.rs` contains a source-derived subset of
`github.com/d2lang/mathjax-go v0.1.0/internal/font/data_normal_gen.go`, itself
generated from MathJax 3.2.2 TeX font data. The upstream generated file and
MathJax sources are Copyright 2017–2022 The MathJax Consortium and are licensed
under the Apache License, Version 2.0. The color extension and named color table also retain Copyright (c) 2018–2022 Omar Al-Ithawi and The MathJax Consortium. Source-derived color/enclosure scopes, padding/strokes and named colors, radical variants/clearance, script bounding boxes/shifts, fraction bounding boxes,
TeX atom-class spacing, script scale/style and binary-number serialization in
`crates/d2-rs-math/src/lib.rs` also translate MathJax via mathjax-go v0.1.0. Those translated
portions, generated glyph metrics, outlines and italic corrections retain
Apache-2.0 terms. Surrounding first-party Rust modifications retain MPL-2.0. The pinned module's `LICENSE`, `NOTICE`, and
`PROVENANCE.md` are the corresponding source and attribution records.

Apache License 2.0:
https://www.apache.org/licenses/LICENSE-2.0

## Shared imageasset SVG XML and codecs

The following notices cover `crates/d2-rs-math/src/image_svg/`.

The XML name character ranges in crates/d2-rs-math/src/image_svg/go_xml_names.rs are translated from
Go 1.27.0 encoding/xml, used by the pinned D2 reference toolchain.
https://go.dev/src/encoding/xml/xml.go

Copyright 2009 The Go Authors.

Redistribution and use in source and binary forms, with or without
modification, are permitted provided that the following conditions are
met:

   * Redistributions of source code must retain the above copyright
notice, this list of conditions and the following disclaimer.
   * Redistributions in binary form must reproduce the above
copyright notice, this list of conditions and the following disclaimer
in the documentation and/or other materials provided with the
distribution.
   * Neither the name of Google LLC nor the names of its
contributors may be used to endorse or promote products derived from
this software without specific prior written permission.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS
"AS IS" AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT
LIMITED TO, THE IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR
A PARTICULAR PURPOSE ARE DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT
OWNER OR CONTRIBUTORS BE LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL,
SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT
LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES; LOSS OF USE,
DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON ANY
THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT
(INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE
OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.

The BSD-3-Clause Go encoding/xml notice above also covers the translated
procInst lookup in crates/d2-rs-math/src/image_svg/svg_import_envelope.rs:decoder_field
(Go 1.27.0). The other envelope admission rules derive from D2 v0.9.0 MPL-2.0
import.go and lib/svg/doctype.go.

The Go encoding/xml BSD-3-Clause notice also covers reachable lexical, entity,
character-range and element-stack translation in
crates/d2-rs-math/src/image_svg/svg_asset_xml.rs (Go 1.27.0). D2 asset root/admission
rules retain MPL-2.0 mapping to lib/imageasset/probe.go.

## TrueType font subsetter

`crates/d2-rs-text/src/font_subset.rs` and the literal function-by-function
port `crates/d2-rs-text/src/subset_font.rs` are Rust translations of D2
v0.9.0's `lib/font/subsetFont.go`, which is derived from
`github.com/jung-kurt/gofpdf`. The translated implementation preserves the
following source notice:

```text
The following code is part of https://github.com/jung-kurt/gofpdf

Copyright (c) 2019 Arteom Korotkiy (Gmail: arteomkorotkiy)

Permission to use, copy, modify, and distribute this software for any
purpose with or without fee is hereby granted, provided that the above
copyright notice and this permission notice appear in all copies.

THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHOR DISCLAIMS ALL WARRANTIES
WITH REGARD TO THIS SOFTWARE INCLUDING ALL IMPLIED WARRANTIES OF
MERCHANTABILITY AND FITNESS. IN NO EVENT SHALL THE AUTHOR BE LIABLE FOR
ANY SPECIAL, DIRECT, INDIRECT, OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES
WHATSOEVER RESULTING FROM LOSS OF USE, DATA OR PROFITS, WHETHER IN AN
ACTION OF CONTRACT, NEGLIGENCE OR OTHER TORTIOUS ACTION, ARISING OUT OF
OR IN CONNECTION WITH THE USE OR PERFORMANCE OF THIS SOFTWARE.
```

Native `crates/d2-rs-export/src/gif_encode.rs` translates Go 1.27
`compress/lzw/writer.go`. Copyright 2011 The Go Authors. The Go BSD-3-Clause
conditions and disclaimer above apply. GIF assembly follows D2 `lib/xgif/encode.go`
under its retained MPL-2.0 terms.

## DejaVu Sans test fonts

crates/d2-rs-cli/tests/fonts/DejaVuSans.ttf and DejaVuSans-Bold.ttf are the
unmodified DejaVu Sans 2.37 fonts, used to make fallback-font regression tests
independent of host fonts.
Source: https://dejavu-fonts.github.io/

Copyright: Copyright (c) 2003 by Bitstream, Inc. All Rights Reserved.
Bitstream Vera is a trademark of Bitstream, Inc.
DejaVu changes are in public domain.

Permission is hereby granted, free of charge, to any person obtaining a copy
of the fonts accompanying this license ("Fonts") and associated
documentation files (the "Font Software"), to reproduce and distribute the
Font Software, including without limitation the rights to use, copy, merge,
publish, distribute, and/or sell copies of the Font Software, and to permit
persons to whom the Font Software is furnished to do so, subject to the
following conditions:

The above copyright and trademark notices and this permission notice shall
be included in all copies of one or more of the Font Software typefaces.

The Font Software may be modified, altered, or added to, and in particular
the designs of glyphs or characters in the Fonts may be modified and
additional glyphs or characters may be added to the Fonts, only if the fonts
are renamed to names not containing either the words "Bitstream" or the word
"Vera".

This License becomes null and void to the extent applicable to Fonts or Font
Software that has been modified and is distributed under the "Bitstream
Vera" names.

The Font Software may be sold as part of a larger software package but no
copy of one or more of the Font Software typefaces may be sold by itself.

THE FONT SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS
OR IMPLIED, INCLUDING BUT NOT LIMITED TO ANY WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT OF COPYRIGHT, PATENT,
TRADEMARK, OR OTHER RIGHT. IN NO EVENT SHALL BITSTREAM OR THE GNOME
FOUNDATION BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, INCLUDING
ANY GENERAL, SPECIAL, INDIRECT, INCIDENTAL, OR CONSEQUENTIAL DAMAGES,
WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF
THE USE OR INABILITY TO USE THE FONT SOFTWARE OR FROM OTHER DEALINGS IN THE
FONT SOFTWARE.

Except as contained in this notice, the names of Gnome, the Gnome
Foundation, and Bitstream Inc., shall not be used in advertising or
otherwise to promote the sale, use or other dealings in this Font Software
without prior written authorization from the Gnome Foundation or Bitstream
Inc., respectively. For further information, contact: fonts at gnome dot
org.
