# Workbench font fallbacks

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
It adds 1,489 mapped characters beyond egui 0.27's bundled Noto Emoji subset,
including modern monochrome emoji. SHA-256:
`8f685aeeef29fd7ab230e1b9aafda3bbc0e3c35c19553038e95d12e468338f07`.
License: `NotoEmoji-OFL.txt` (SIL OFL 1.1).

Together the bundled assets add about 16.6 MiB to the executable. They do not
cover every Unicode script or uncommon ideograph. Egui 0.27 renders glyph
outlines as monochrome; it cannot render Noto Color Emoji's bitmap strikes or
compose multi-codepoint emoji sequences such as family, flag, or skin-tone
sequences. Both assets remain last in the fallback chains, preserving Latin
typography and egui's existing symbol/emoji choices where those fonts suffice.
