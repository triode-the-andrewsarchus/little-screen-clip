# Shared screen and workbench font fallbacks

These canonical assets serve both raster and native desktop presentation.
Fallback font bytes are owned by `little-clip-core::font_assets` and reexported
by `little-clip-render`, so `clip-ui` does not depend on the raster/scene renderer. The screen renderer uses them only when its selected system
font lacks a glyph, with the same selection used for measurement and drawing.
Parsed fallback fonts are shared across renderers and loaded on first use.

`NotoSansCJK-JP-Regular.ttf` is the JP regular face extracted from Google's
Noto Sans CJK collection distributed in Arch's `noto-fonts-cjk` 20240730
package. It is embedded after egui's built-in Latin, symbol, and emoji fonts
in both proportional and monospace families, so CJK text renders without
depending on fonts installed on the host. Its 44,810 mapped characters cover
Japanese, Simplified/Traditional Chinese, Korean, and shared CJK symbols.
SHA-256: `86f7ba3101617c899dd2ea44c51f0d58e58892709949c8619f5800ada8d9b601`.
License: `NotoSansCJK-OFL.txt` (SIL OFL 1.1).

`NotoEmoji-Regular.ttf` is a static Regular instance (`wght=400`) of Google's
variable Noto Emoji v3.002, generated with fontTools 4.66.0 without subsetting
and pinned at commit
[8b0a1d0](https://github.com/google/fonts/tree/8b0a1d0f5983c89bc2b93f1b5fb55f9e252744b5/ofl/notoemoji).
It supplies modern monochrome emoji in portable installations. SHA-256
`8f685aeeef29fd7ab230e1b9aafda3bbc0e3c35c19553038e95d12e468338f07`.
License is `NotoEmoji-OFL.txt` (SIL OFL 1.1).

Together the bundled assets add about 16.6 MiB to each executable using them.
They follow the normal Latin fonts in the proportional and monospace families
to keep those metrics stable. Desktop eframe 0.37 can also find system fonts
for missing glyphs, including installed color emoji fonts. The available glyphs
depend on the system font inventory.

`FiraSans-Regular.ttf` supplies the shared desktop Latin face. Its original
license is `FiraSans-OFL.txt`; moving the asset here does not change the font.
