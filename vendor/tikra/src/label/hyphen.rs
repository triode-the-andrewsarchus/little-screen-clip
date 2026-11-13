//! Where TeX may hyphenate an English word: Liang's algorithm, with the patterns and exceptions
//! of Knuth's `hyphen.tex` (in `hyphen-en.txt`, with that file's notice), which LaTeX's default
//! language loads.

use std::collections::HashMap;
use std::sync::OnceLock;

const DATA: &str = include_str!("hyphen-en.txt");

struct Patterns {
    /// A pattern's letters (`.` for a word's edge) and the values between them, one more than the
    /// letters: `a1b` is `ab` with `[0, 1, 0]`.
    values: HashMap<&'static str, Vec<u8>>,
    longest: usize,
    /// A word of the exception list and its hyphens, as positions to break after.
    exceptions: HashMap<String, Vec<usize>>,
}

fn patterns() -> &'static Patterns {
    static PATTERNS: OnceLock<Patterns> = OnceLock::new();
    PATTERNS.get_or_init(|| {
        let body = DATA.split_once("\npatterns\n").map_or("", |(_, b)| b);
        let (list, exceptions) = body.split_once("\nexceptions\n").unwrap_or((body, ""));
        let mut values = HashMap::new();
        let mut longest = 0;
        for p in list.split_whitespace() {
            let letters_at = p.char_indices().filter(|(_, c)| !c.is_ascii_digit());
            let key_len = letters_at.clone().count();
            let mut v = vec![0u8; key_len + 1];
            let mut k = 0;
            for c in p.chars() {
                match c.to_digit(10) {
                    Some(d) => v[k] = d as u8,
                    None => k += 1,
                }
            }
            // the letters: the pattern without its digits, which are ASCII, kept as a slice of
            // the data where it has none
            let key: &'static str = if key_len == p.len() { p } else { Box::leak(p.chars().filter(|c| !c.is_ascii_digit()).collect::<String>().into_boxed_str()) };
            longest = longest.max(key.len());
            values.insert(key, v);
        }
        let exceptions = exceptions
            .split_whitespace()
            .map(|w| {
                let mut breaks = Vec::new();
                let mut n = 0;
                for c in w.chars() {
                    if c == '-' {
                        breaks.push(n);
                    } else {
                        n += 1;
                    }
                }
                (w.replace('-', ""), breaks)
            })
            .collect();
        Patterns { values, longest, exceptions }
    })
}

/// The positions (in letters from the start) after which TeX may break `word`, all letters, with
/// a hyphen: at least `left` letters before the break and `right` after (`\lefthyphenmin` and
/// `\righthyphenmin`).
pub fn hyphenate(word: &str, left: usize, right: usize) -> Vec<usize> {
    let lower = word.to_ascii_lowercase();
    let n = lower.len();
    if !lower.bytes().all(|b| b.is_ascii_lowercase()) || n < left + right {
        return Vec::new();
    }
    let p = patterns();
    let keep = |k: &usize| *k >= left && n - *k >= right;
    if let Some(breaks) = p.exceptions.get(&lower) {
        return breaks.iter().copied().filter(keep).collect();
    }
    let w = format!(".{lower}.");
    // the value between letters i - 1 and i of `w`
    let mut points = vec![0u8; w.len() + 1];
    for i in 0..w.len() {
        for j in i + 1..=(i + p.longest).min(w.len()) {
            if let Some(v) = p.values.get(&w[i..j]) {
                for (k, &d) in v.iter().enumerate() {
                    points[i + k] = points[i + k].max(d);
                }
            }
        }
    }
    // after letter k of the word: between letters k and k + 1 of `w`
    (1..n).filter(|k| points[k + 1] % 2 == 1).filter(keep).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hyphenated(w: &str) -> String {
        let mut out = String::new();
        let breaks = hyphenate(w, 2, 3);
        for (k, c) in w.chars().enumerate() {
            if breaks.contains(&k) {
                out.push('-');
            }
            out.push(c);
        }
        out
    }

    #[test]
    fn as_tex_hyphenates() {
        assert_eq!(hyphenated("demonstration"), "demon-stra-tion");
        assert_eq!(hyphenated("hyphenation"), "hy-phen-ation");
        assert_eq!(hyphenated("Randomization"), "Ran-dom-iza-tion");
        assert_eq!(hyphenated("table"), "ta-ble");
        assert_eq!(hyphenated("the"), "the");
    }
}
