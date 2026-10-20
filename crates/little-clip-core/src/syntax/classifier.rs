use flate2::read::GzDecoder;
use serde::Deserialize;
use std::collections::HashMap;
use std::io::Read;
use std::sync::LazyLock;

#[derive(Deserialize)]
struct LanguageModel {
    rank: u32,
    tokens: HashMap<String, f32>,
}

pub struct SyntaxClassifier {
    models: HashMap<String, LanguageModel>,
}

static CLASSIFIER: LazyLock<SyntaxClassifier> = LazyLock::new(|| {
    let compressed = include_bytes!("classifier_data.bin.gz");
    let mut decoder = GzDecoder::new(&compressed[..]);
    let mut decompressed = Vec::new();
    decoder
        .read_to_end(&mut decompressed)
        .expect("valid classifier data");
    let models: HashMap<String, LanguageModel> =
        serde_json::from_slice(&decompressed).expect("valid classifier json");
    SyntaxClassifier { models }
});

const PUNCT_STOPWORDS: &[&str] = &[";", "{", "}", "(", ")", "[", "]", ",", ".", ":", "\"", "'"];

const PROSE_STOPWORDS: &[&str] = &[
    "the", "The", "THE", "a", "A", "an", "An", "and", "And", "or", "Or", "but", "But", "in", "on",
    "at", "to", "for", "of", "with", "by", "from", "up", "about", "into", "over", "after",
    "through", "is", "are", "was", "were", "be", "been", "being", "have", "has", "had", "do",
    "does", "did", "will", "would", "shall", "should", "can", "could", "may", "might", "must",
    "that", "That", "which", "who", "whom", "this", "This", "these", "those", "it", "It", "its",
    "they", "them", "their", "we", "us", "our", "you", "your", "he", "him", "his", "she", "her",
    "i", "me", "my", "walk", "garden", "lovely", "another", "Another", "notes", "Notes", "plain",
    "Plain", "day", "Day", "time", "Time", "year", "years", "people", "way", "good", "man",
];

impl SyntaxClassifier {
    pub fn classify(&self, text: &str) -> Option<&'static str> {
        let tokens = tokenize(text);
        if tokens.is_empty() {
            return None;
        }

        let mut tf: HashMap<&str, usize> = HashMap::new();
        for &t in &tokens {
            if !PROSE_STOPWORDS.contains(&t) {
                *tf.entry(t).or_default() += 1;
            }
        }

        let has_code_symbol = text.contains('{')
            || text.contains('}')
            || text.contains(';')
            || text.contains("->")
            || text.contains("=>")
            || text.contains("::")
            || text.contains("==")
            || text.contains("!=");

        let mut candidate_scores: Vec<(f32, u32, &str)> = Vec::with_capacity(self.models.len());

        for (lang_code, model) in &self.models {
            let mut score = 0.0f32;
            let mut word_matches = 0;
            let mut keyword_matches = 0;

            for (&token, &count) in &tf {
                if let Some(&log_prob) = model.tokens.get(token) {
                    if log_prob >= -0.2 {
                        keyword_matches += 1;
                    }
                    let mut weight = 10.0 + log_prob;
                    if !PUNCT_STOPWORDS.contains(&token) {
                        weight *= 2.5;
                        word_matches += 1;
                    }
                    if weight > 0.0 {
                        score += weight * (1.0 + (count as f32).ln());
                    }
                }
            }

            // Require at least one distinctive keyword (log_prob >= -0.2) or code symbols
            // to prevent random English dictionary overlaps from classifying prose as code.
            if (keyword_matches > 0 || has_code_symbol) && word_matches >= 2 && score >= 20.0 {
                candidate_scores.push((score, model.rank, lang_code.as_str()));
            }
        }

        if candidate_scores.is_empty() {
            return None;
        }

        candidate_scores.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));

        let best_score = candidate_scores[0].0;
        let threshold = best_score * 0.95;

        candidate_scores
            .into_iter()
            .take_while(|item| item.0 >= threshold)
            .max_by_key(|item| (item.1, (item.0 * 100.0) as i32))
            .map(|(_, _, lang)| to_static_lang(lang))
    }
}

fn tokenize(text: &str) -> Vec<&str> {
    let mut tokens = Vec::new();
    let bytes = text.as_bytes();
    let len = bytes.len();
    let mut i = 0;

    while i < len {
        let b = bytes[i];

        // Skip whitespace
        if b.is_ascii_whitespace() {
            i += 1;
            continue;
        }

        // Skip single-line comments
        if (b == b'/' && i + 1 < len && bytes[i + 1] == b'/') || b == b'#' {
            while i < len && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        }

        // String literals: do not trigger on contractions/apostrophes (e.g. don't, it's, Vega's)
        if b == b'"' || b == b'\'' {
            let is_contraction = b == b'\''
                && i > 0
                && (bytes[i - 1].is_ascii_alphanumeric() || bytes[i - 1] == b'_')
                && i + 1 < len
                && bytes[i + 1].is_ascii_alphabetic();

            if is_contraction {
                i += 1;
                continue;
            }

            let quote = b;
            i += 1;
            // Stop at closing quote, or at newline (unmatched quote should not swallow entire document)
            while i < len && bytes[i] != quote && bytes[i] != b'\n' {
                if bytes[i] == b'\\' && i + 1 < len {
                    i += 2;
                } else {
                    i += 1;
                }
            }
            if i < len && bytes[i] == quote {
                i += 1;
            }
            continue;
        }

        // Punctuation
        if matches!(b, b';' | b'{' | b'}' | b'(' | b')' | b'[' | b']') {
            tokens.push(&text[i..i + 1]);
            i += 1;
            continue;
        }

        // Operators
        if matches!(b, b'+' | b'*' | b'/' | b'%' | b'&' | b'|' | b'<' | b'>') {
            let start = i;
            i += 1;
            if i < len
                && (bytes[i] == b'<' || bytes[i] == b'&' || bytes[i] == b'|' || bytes[i] == b'=')
            {
                i += 1;
            }
            tokens.push(&text[start..i]);
            continue;
        }

        // Skip Unicode curly apostrophe (’ U+2019: 0xE2 0x80 0x99) inside words
        if b == 0xE2 && i + 2 < len && bytes[i + 1] == 0x80 && bytes[i + 2] == 0x99 {
            i += 3;
            continue;
        }

        // Regular word tokens (alphanumeric, _, or @ / - prefix for directives)
        if b.is_ascii_alphanumeric()
            || b == b'_'
            || ((b == b'@' || b == b'-') && i + 1 < len && bytes[i + 1].is_ascii_alphabetic())
        {
            let start = i;
            if b == b'@' || b == b'-' {
                i += 1;
            }
            while i < len && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                i += 1;
            }
            // Check for Rust macro exclamation mark: println!, vec!, etc.
            if i < len
                && bytes[i] == b'!'
                && matches!(
                    &text[start..i],
                    "println" | "eprintln" | "vec" | "format" | "panic"
                )
            {
                i += 1;
            }
            tokens.push(&text[start..i]);
            continue;
        }

        i += 1;
    }

    tokens
}

fn to_static_lang(s: &str) -> &'static str {
    match s {
        "rs" => "rs",
        "py" => "py",
        "c" => "c",
        "cpp" => "cpp",
        "cs" => "cs",
        "go" => "go",
        "js" => "js",
        "ts" => "ts",
        "java" => "java",
        "kt" => "kt",
        "swift" => "swift",
        "dart" => "dart",
        "rb" => "rb",
        "php" => "php",
        "sh" => "sh",
        "bat" => "bat",
        "ps1" => "ps1",
        "sql" => "sql",
        "html" => "html",
        "css" => "css",
        "pl" => "pl",
        "r" => "r",
        "jl" => "jl",
        "scala" => "scala",
        "hs" => "hs",
        "ex" => "ex",
        "erl" => "erl",
        "clj" => "clj",
        "zig" => "zig",
        "lua" => "lua",
        "d" => "d",
        "groovy" => "groovy",
        "ml" => "ml",
        "fs" => "fs",
        "lisp" => "lisp",
        "scm" => "scm",
        "janet" => "janet",
        "pascal" => "pascal",
        "fortran" => "fortran",
        "asm" => "asm",
        "nim" => "nim",
        "cr" => "cr",
        "odin" => "odin",
        "red" => "red",
        "cobol" => "cobol",
        "forth" => "forth",
        "vb" => "vb",
        "apl" => "apl",
        "tcl" => "tcl",
        "ahk" => "ahk",
        "moon" => "moon",
        "wren" => "wren",
        "coffee" => "coffee",
        "raku" => "raku",
        "elm" => "elm",
        "purs" => "purs",
        "re" => "re",
        "idr" => "idr",
        "agda" => "agda",
        "lean" => "lean",
        "futhark" => "futhark",
        "koka" => "koka",
        "roc" => "roc",
        "vhdl" => "vhdl",
        "verilog" => "verilog",
        "wolfram" => "wolfram",
        "move" => "move",
        "proto" => "proto",
        "thrift" => "thrift",
        "graphql" => "graphql",
        "tf" => "tf",
        "nix" => "nix",
        "cmake" => "cmake",
        "vim" => "vim",
        "awk" => "awk",
        "glsl" => "glsl",
        "hlsl" => "hlsl",
        "metal" => "metal",
        "tex" => "tex",
        "bf" => "bf",
        "wat" => "wat",
        "fish" => "fish",
        "nu" => "nu",
        "csh" => "csh",
        "hack" => "hack",
        "sol" => "sol",
        "hx" => "hx",
        "llvm" => "llvm",
        _ => "txt",
    }
}

pub fn classify_snippet(text: &str) -> Option<&'static str> {
    CLASSIFIER.classify(text)
}
