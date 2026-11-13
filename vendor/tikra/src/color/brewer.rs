//! The ColorBrewer schemes (`\usetikzlibrary{colorbrewer}`, `\usepgfplotslibrary{colorbrewer}`):
//! colors `PuBu-A` … `PuBu-M`, and colormaps `PuBu-3` … `PuBu-9` (`PuBu`, the largest).
//!
//! Each scheme lists its distinct colors under the letters ColorBrewer gives them, darkest or
//! lightest first; a scheme of `n` classes takes some of the letters, the same for every scheme of
//! a kind. Color specifications and designs developed by Cynthia Brewer (http://colorbrewer.org/),
//! under the Apache License 2.0.

use super::Color;

/// How a kind of scheme picks its letters for 3, 4, … classes.
#[derive(Clone, Copy)]
enum Kind {
    Sequential,
    Diverging,
    /// Qualitative: the first `n` colors.
    Qualitative,
}

const SEQUENTIAL: [&str; 7] = ["CFI", "BEGJ", "BEGIK", "BDFGIK", "BDFGHJL", "ACDFGHJL", "ACDFGHJKM"];
const DIVERGING: [&str; 9] = ["EHK", "CFJM", "CFHJM", "BEGIKN", "BEGHIKN", "BDFGIJLN", "BDFGHIJLN", "ABDFGIJLNO", "ABDFGHIJLNO"];

/// Each scheme: its name, kind and colors (RGB in hexadecimal) in the order of their letters.
const SCHEMES: &[(&str, Kind, &str)] = &[
    ("Accent", Kind::Qualitative, "7fc97f beaed4 fdc086 ffff99 386cb0 f0027f bf5b17 666666"),
    ("Blues", Kind::Sequential, "f7fbff eff3ff deebf7 c6dbef bdd7e7 9ecae1 6baed6 4292c6 3182bd 2171b5 08519c 084594 08306b"),
    ("BrBG", Kind::Diverging, "543005 8c510a a6611a bf812d d8b365 dfc27d f6e8c3 f5f5f5 c7eae5 80cdc1 5ab4ac 35978f 018571 01665e 003c30"),
    ("BuGn", Kind::Sequential, "f7fcfd edf8fb e5f5f9 ccece6 b2e2e2 99d8c9 66c2a4 41ae76 2ca25f 238b45 006d2c 005824 00441b"),
    ("BuPu", Kind::Sequential, "f7fcfd edf8fb e0ecf4 bfd3e6 b3cde3 9ebcda 8c96c6 8c6bb1 8856a7 88419d 810f7c 6e016b 4d004b"),
    ("Dark2", Kind::Qualitative, "1b9e77 d95f02 7570b3 e7298a 66a61e e6ab02 a6761d 666666"),
    ("GnBu", Kind::Sequential, "f7fcf0 f0f9e8 e0f3db ccebc5 bae4bc a8ddb5 7bccc4 4eb3d3 43a2ca 2b8cbe 0868ac 08589e 084081"),
    ("Greens", Kind::Sequential, "f7fcf5 edf8e9 e5f5e0 c7e9c0 bae4b3 a1d99b 74c476 41ab5d 31a354 238b45 006d2c 005a32 00441b"),
    ("Greys", Kind::Sequential, "ffffff f7f7f7 f0f0f0 d9d9d9 cccccc bdbdbd 969696 737373 636363 525252 252525 252525 000000"),
    ("OrRd", Kind::Sequential, "fff7ec fef0d9 fee8c8 fdd49e fdcc8a fdbb84 fc8d59 ef6548 e34a33 d7301f b30000 990000 7f0000"),
    ("Oranges", Kind::Sequential, "fff5eb feedde fee6ce fdd0a2 fdbe85 fdae6b fd8d3c f16913 e6550d d94701 a63603 8c2d04 7f2704"),
    ("PRGn", Kind::Diverging, "40004b 762a83 7b3294 9970ab af8dc3 c2a5cf e7d4e8 f7f7f7 d9f0d3 a6dba0 7fbf7b 5aae61 008837 1b7837 00441b"),
    ("Paired", Kind::Qualitative, "a6cee3 1f78b4 b2df8a 33a02c fb9a99 e31a1c fdbf6f ff7f00 cab2d6 6a3d9a ffff99 b15928"),
    ("Pastel1", Kind::Qualitative, "fbb4ae b3cde3 ccebc5 decbe4 fed9a6 ffffcc e5d8bd fddaec f2f2f2"),
    ("Pastel2", Kind::Qualitative, "b3e2cd fdcdac cbd5e8 f4cae4 e6f5c9 fff2ae f1e2cc cccccc"),
    ("PiYG", Kind::Diverging, "8e0152 c51b7d d01c8b de77ae e9a3c9 f1b6da fde0ef f7f7f7 e6f5d0 b8e186 a1d76a 7fbc41 4dac26 4d9221 276419"),
    ("PuBu", Kind::Sequential, "fff7fb f1eef6 ece7f2 d0d1e6 bdc9e1 a6bddb 74a9cf 3690c0 2b8cbe 0570b0 045a8d 034e7b 023858"),
    ("PuBuGn", Kind::Sequential, "fff7fb f6eff7 ece2f0 d0d1e6 bdc9e1 a6bddb 67a9cf 3690c0 1c9099 02818a 016c59 016450 014636"),
    ("PuOr", Kind::Diverging, "2d004b 542788 5e3c99 8073ac 998ec3 b2abd2 d8daeb f7f7f7 fee0b6 fdb863 f1a340 e08214 e66101 b35806 7f3b08"),
    ("PuRd", Kind::Sequential, "f7f4f9 f1eef6 e7e1ef d4b9da d7b5d8 c994c7 df65b0 e7298a dd1c77 ce1256 980043 91003f 67001f"),
    ("Purples", Kind::Sequential, "fcfbfd f2f0f7 efedf5 dadaeb cbc9e2 bcbddc 9e9ac8 807dba 756bb1 6a51a3 54278f 4a1486 3f007d"),
    ("RdBu", Kind::Diverging, "67001f b2182b ca0020 d6604d ef8a62 f4a582 fddbc7 f7f7f7 d1e5f0 92c5de 67a9cf 4393c3 0571b0 2166ac 053061"),
    ("RdGy", Kind::Diverging, "67001f b2182b ca0020 d6604d ef8a62 f4a582 fddbc7 ffffff e0e0e0 bababa 999999 878787 404040 4d4d4d 1a1a1a"),
    ("RdPu", Kind::Sequential, "fff7f3 feebe2 fde0dd fcc5c0 fbb4b9 fa9fb5 f768a1 dd3497 c51b8a ae017e 7a0177 7a0177 49006a"),
    ("RdYlBu", Kind::Diverging, "a50026 d73027 d7191c f46d43 fc8d59 fdae61 fee090 ffffbf e0f3f8 abd9e9 91bfdb 74add1 2c7bb6 4575b4 313695"),
    ("RdYlGn", Kind::Diverging, "a50026 d73027 d7191c f46d43 fc8d59 fdae61 fee08b ffffbf d9ef8b a6d96a 91cf60 66bd63 1a9641 1a9850 006837"),
    ("Reds", Kind::Sequential, "fff5f0 fee5d9 fee0d2 fcbba1 fcae91 fc9272 fb6a4a ef3b2c de2d26 cb181d a50f15 99000d 67000d"),
    ("Set1", Kind::Qualitative, "e41a1c 377eb8 4daf4a 984ea3 ff7f00 ffff33 a65628 f781bf 999999"),
    ("Set2", Kind::Qualitative, "66c2a5 fc8d62 8da0cb e78ac3 a6d854 ffd92f e5c494 b3b3b3"),
    ("Set3", Kind::Qualitative, "8dd3c7 ffffb3 bebada fb8072 80b1d3 fdb462 b3de69 fccde5 d9d9d9 bc80bd ccebc5 ffed6f"),
    ("Spectral", Kind::Diverging, "9e0142 d53e4f d7191c f46d43 fc8d59 fdae61 fee08b ffffbf e6f598 abdda4 99d594 66c2a5 2b83ba 3288bd 5e4fa2"),
    ("YlGn", Kind::Sequential, "ffffe5 ffffcc f7fcb9 d9f0a3 c2e699 addd8e 78c679 41ab5d 31a354 238443 006837 005a32 004529"),
    ("YlGnBu", Kind::Sequential, "ffffd9 ffffcc edf8b1 c7e9b4 a1dab4 7fcdbb 41b6c4 1d91c0 2c7fb8 225ea8 253494 0c2c84 081d58"),
    ("YlOrBr", Kind::Sequential, "ffffe5 ffffd4 fff7bc fee391 fed98e fec44f fe9929 ec7014 d95f0e cc4c02 993404 8c2d04 662506"),
    ("YlOrRd", Kind::Sequential, "ffffcc ffffb2 ffeda0 fed976 fecc5c feb24c fd8d3c fc4e2a f03b20 e31a1c bd0026 b10026 800026"),
];

fn hex(h: &str) -> Option<Color> {
    let v = |i: usize| u8::from_str_radix(h.get(i..i + 2)?, 16).ok().map(|x| f64::from(x) / 255.0);
    Some(Color::rgb(v(0)?, v(2)?, v(4)?))
}

fn scheme(name: &str) -> Option<(Kind, Vec<&'static str>)> {
    SCHEMES.iter().find(|(n, _, _)| *n == name).map(|(_, k, c)| (*k, c.split(' ').collect()))
}

/// A color such as `PuBu-C`.
pub fn color(name: &str) -> Option<Color> {
    let (s, letter) = name.rsplit_once('-')?;
    let mut l = letter.chars();
    let (Some(c @ 'A'..='Z'), None) = (l.next(), l.next()) else { return None };
    let (_, colors) = scheme(s)?;
    hex(colors.get((c as u8 - b'A') as usize)?)
}

/// The colors of a colormap such as `PuBu-6`, or `PuBu` (the most classes).
pub fn colormap(name: &str) -> Option<Vec<Color>> {
    let (s, n) = match name.rsplit_once('-') {
        Some((s, n)) => (s, Some(n.parse::<usize>().ok()?)),
        None => (name, None),
    };
    let (kind, colors) = scheme(s)?;
    let letters: Vec<usize> = match kind {
        Kind::Qualitative => (0..n.unwrap_or(colors.len())).collect(),
        Kind::Sequential | Kind::Diverging => {
            let table: &[&str] = if matches!(kind, Kind::Sequential) { &SEQUENTIAL } else { &DIVERGING };
            let k = n.unwrap_or(table.len() + 2);
            table.get(k.checked_sub(3)?)?.bytes().map(|b| (b - b'A') as usize).collect()
        }
    };
    if n.is_some_and(|n| n < 3 || n > letters.len()) {
        return None;
    }
    letters.iter().map(|&i| colors.get(i).and_then(|h| hex(h))).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn letters_and_classes() {
        assert_eq!(color("RdPu-B").unwrap().hex(), "#feebe2");
        assert_eq!(colormap("YlOrRd").unwrap().iter().map(Color::hex).collect::<Vec<_>>()[..3], ["#ffffcc", "#ffeda0", "#fed976"]);
        assert_eq!(colormap("RdPu-6").unwrap().len(), 6);
        assert_eq!(colormap("PuOr").unwrap().len(), 11);
        assert_eq!(colormap("Set1-4").unwrap().len(), 4);
        assert!(colormap("PuBu-12").is_none());
        assert!(color("PuBu-Z").is_none());
    }
}
