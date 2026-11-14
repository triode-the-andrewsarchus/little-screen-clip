# Xournal fixtures

Unmodified public test files from the GPL-2.0-or-later Xournal++ repository,
commit `4d6810b3760c7b3f90c6a3271d6a5623cfb5481f`. The upstream license is retained as
`LICENSE-GPL-2.0`. No upstream implementation code is imported here.

| Local file | Upstream path | SHA-256 |
| --- | --- | --- |
| `load/strokes.xopp` | `test/files/load/strokes.xopp` | `dc48fb9c491a9491a80e9c070bf2d48e8f31cc51f8131038262c3a35ae22369c` |
| `load/layers.xopp` | `test/files/load/layers.xopp` | `c3982354141079ab41dc0559430dd9624d5d87694605d0059805dbe56ee092ea` |
| `load/pages.xopp` | `test/files/load/pages.xopp` | `4fa96c1c47cd9c95c3b139f57b7e5ad9b393bd38e0cb3c8373172ea00f1fc5d4` |
| `load/text-fileversion-5.xopp` | `test/files/load/text-fileversion-5.xopp` | `7b1605ac7450bf727188be1e6e70537d8d550e66b8e966431ba36baebd19bd5d` |
| `load/image-fileversion-5.xopp` | `test/files/load/image-fileversion-5.xopp` | `b733846b2237f1062ed48aad43281c355b04a4efbf96199aa348d014ce7862ec` |
| `load/relativePaths.xopp` | `test/files/load/relativePaths.xopp` | `3a401e7fabca5a4196f7d34fb542c145ec6c561b740615c5cb23a2bbb2015ea0` |
| `load/pages.xopp.bg_1.png` | `test/files/load/pages.xopp.bg_1.png` | `4788bd7486e4757d7e66282e9afe3a42e97628c2292c2771b9c437714556952c` |
| `cjk/测试.pdf` | `test/files/cjk/测试.pdf` | `165144624354b52ff5c38f015a4671d687714b7090683df38a06e321cb53cca5` |
| `legacy-stroke.xoj` | `test/files/load/compatibility/stroke_line_return.xoj` | `5bdd58efe079b70302b02ab86dad45f413f5fa079852b32848edc9afd0ddca59` |
| `packaged/stroke.xopp` | `test/files/packaged_xopp/stroke/new.xopp` | `e220331868b125920bc307021b5080f6990296420b484e910932cb916aa1c0dd` |
| `packaged/pdf-background.xopp` | `test/files/packaged_xopp/pdfBackground/new.xopp` | `8237a4a44fbf7477c570bbbd4df11a5bc92468a36f575329eb9b917c97caaf5d` |

## Independent render references

The PNGs in `reference/` are canonical Xournal++ CLI exports at 800 pixels
height, produced by the official 2026-11-11 nightly AppImage
`xournalpp-1.3.8+dev-nightly-20261111-x86_64.AppImage` (SHA-256
`bade9f31044835f2d84d41246b19e3a74e0384ffdf5a8b60a721919d13e60b7f`). They are not renders from
the Rust implementation. `reference-exports.json` records each original source,
page number, image hash and renderer diagnostic. Reproduce with
`scripts/notebook_xournal_reference.py --binary PATH_TO_XOURNALPP --output-dir DIR`.
The CLI is a developer oracle only, not a runtime dependency.

Stable 1.3.8 cannot load the current transformed text/image fixture layout;
the nightly can. Its legacy packaged PDF fixture contains `pageno="1ll"` and
`"2ll"`; upstream warns about trailing characters but renders pages 1 and 2.
Keep that compatibility evidence distinct from a clean modern integer field.

Reference font policy: exports use a private Fontconfig configuration pointing
only at the application's bundled fonts. Generic Sans/Serif/Monospace and their
metric equivalents select Arimo/Tinos/Cousine, with the existing CJK fallback.
This removes the host desktop's font substitutions from layout comparisons; it
does not modify Xournal++ or any document bytes. The export script records the
policy and does not alter user font settings. Earlier exploratory exports used
the host's Noto Sans default and are not the committed comparison baseline.

Additional unmodified qualification fixtures from the same pinned revision:

- `packaged/image.xopp` from `test/files/packaged_xopp/imgAttachment/new.xopp`, SHA-256 `3eb290d1b764d2f4ee1c273209b77fe44d15bcb5e7ddd774ad4dafc72b8b1524`.
- `load/latex-fileversion-5.xopp` from `test/files/load/latex-fileversion-5.xopp`, SHA-256 `e74e9dacf6f586437b9797112aca11fb65a8e8e585d4f78c53eb1d7649cfa37a`.
