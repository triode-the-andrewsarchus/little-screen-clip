//! Programming and markup language detection heuristics.
//!
//! Classifies plain text and code snippets into language tags (`rs`, `py`, `c`, `cpp`, `java`, `sh`, `json`, etc.)
//! using file extension hints, shebang lines, structural markers, and precomputed Bayesian token frequencies.

use std::path::Path;

#[path = "syntax/classifier.rs"]
mod classifier;

pub use classifier::classify_snippet;

/// Detects the programming or markup language from an optional file extension hint
/// and content heuristics, prioritizing common languages: Python, C/C++, Rust, Java, Bash, etc.
pub fn detect_language(content: &str, ext_hint: Option<&str>) -> &'static str {
    // 1. If explicit extension hint is provided, normalize and map it
    if let Some(lang) = check_extension_hint(ext_hint) {
        return lang;
    }

    let trimmed = content.trim();
    if trimmed.is_empty() {
        return "txt";
    }

    // 2. Shebang line check (e.g. #!/usr/bin/env python3, #!/bin/bash)
    if let Some(lang) = check_shebang(trimmed) {
        return lang;
    }

    // 3. Fast structural checks
    if let Some(lang) = check_structural_format(trimmed) {
        return lang;
    }

    // 4. Prose check (detects natural language sentences, contractions, and prose words)
    if is_prose_text(trimmed) {
        return "txt";
    }

    // 5. Precomputed statistical classifier with popularity prior (trained on Linguist corpus)
    if let Some(lang) = classify_snippet(trimmed) {
        return lang;
    }

    // 6. Fallback heuristics for short snippets
    check_snippet_fallback(trimmed).unwrap_or("txt")
}

fn check_extension_hint(ext_hint: Option<&str>) -> Option<&'static str> {
    let ext = ext_hint?;
    let clean = ext.trim().trim_start_matches('.').to_lowercase();
    map_extension_to_language(&clean)
}

fn check_shebang(trimmed: &str) -> Option<&'static str> {
    if !trimmed.starts_with("#!") {
        return None;
    }
    let first_line = trimmed.lines().next()?.to_lowercase();
    if first_line.contains("python") {
        Some("py")
    } else if first_line.contains("bash")
        || first_line.contains("/sh")
        || first_line.contains("zsh")
    {
        Some("sh")
    } else if first_line.contains("node")
        || first_line.contains("deno")
        || first_line.contains("bun")
    {
        Some("js")
    } else if first_line.contains("ruby") {
        Some("rb")
    } else if first_line.contains("perl") {
        Some("pl")
    } else if first_line.contains("elixir") {
        Some("ex")
    } else if first_line.contains("julia") {
        Some("jl")
    } else if first_line.contains("pwsh") || first_line.contains("powershell") {
        Some("ps1")
    } else if first_line.contains("rscript") {
        Some("r")
    } else if first_line.contains("groovy") {
        Some("groovy")
    } else if first_line.contains("scala") {
        Some("scala")
    } else if first_line.contains("fish") {
        Some("fish")
    } else if first_line.contains("nu") {
        Some("nu")
    } else if first_line.contains("murex") {
        Some("sh")
    } else if first_line.contains("lua") {
        Some("lua")
    } else if first_line.contains("tcl") || first_line.contains("wish") {
        Some("tcl")
    } else if first_line.contains("awk") {
        Some("awk")
    } else {
        None
    }
}

fn check_structural_format(trimmed: &str) -> Option<&'static str> {
    if ((trimmed.starts_with('{') && trimmed.ends_with('}'))
        || (trimmed.starts_with('[') && trimmed.ends_with(']')))
        && serde_json::from_str::<serde_json::Value>(trimmed).is_ok()
    {
        return Some("json");
    }

    if trimmed.starts_with("<!DOCTYPE")
        || trimmed.starts_with("<html")
        || trimmed.starts_with("<?xml")
    {
        return Some("html");
    }

    if trimmed.starts_with("---") && trimmed.contains(": ") {
        return Some("yaml");
    }

    if (trimmed.starts_with("[package]")
        || trimmed.starts_with("[dependencies]")
        || trimmed.starts_with("[workspace]"))
        && trimmed.contains('=')
    {
        return Some("toml");
    }

    None
}

fn check_snippet_fallback(trimmed: &str) -> Option<&'static str> {
    if trimmed.contains("fn ") || trimmed.contains("println!") {
        Some("rs")
    } else if trimmed.contains("def ") || trimmed.contains("print(") {
        Some("py")
    } else if trimmed.contains("#include") || trimmed.contains("printf(") {
        Some("c")
    } else if trimmed.contains("echo ") || trimmed.contains("export ") {
        Some("sh")
    } else if trimmed.contains("public class") || trimmed.contains("System.out.") {
        Some("java")
    } else if trimmed.contains("Console.WriteLine") || trimmed.contains("using System;") {
        Some("cs")
    } else if trimmed.contains("Write-Host") || trimmed.contains("Get-Process") {
        Some("ps1")
    } else if trimmed.contains("@echo off") {
        Some("bat")
    } else if trimmed.contains("defmodule ") {
        Some("ex")
    } else if trimmed.contains("-module(") {
        Some("erl")
    } else if trimmed.contains("(defn ") || trimmed.contains("(ns ") {
        Some("clj")
    } else if trimmed.contains("putStrLn ") {
        Some("hs")
    } else if trimmed.contains("@import(\"std\")") {
        Some("zig")
    } else if trimmed.contains("import std.stdio;") {
        Some("d")
    } else if trimmed.contains("fun ") {
        Some("kt")
    } else {
        None
    }
}

/// Returns true if the text exhibits clear prose structure without code characteristics.
pub fn is_prose_text(trimmed: &str) -> bool {
    // 1. Structural code symbols
    // Code almost universally uses braces, statement terminators, or code operators.
    const CODE_SYMBOLS: &[&str] = &[
        "{", "}", ";", "->", "=>", "::", "==", "!=", ":=", "+=", "-=", "*=", "/=", "&&", "||",
        "<<", ">>",
    ];
    if CODE_SYMBOLS.iter().any(|&s| trimmed.contains(s)) {
        return false;
    }

    // 2. Line-start code prefixes / keywords
    const CODE_LINE_PREFIXES: &[&str] = &[
        "fn ",
        "pub ",
        "def ",
        "class ",
        "import ",
        "from ",
        "#include",
        "include ",
        "package ",
        "func ",
        "var ",
        "val ",
        "let ",
        "const ",
        "struct ",
        "enum ",
        "type ",
        "interface ",
        "pragma ",
        "SELECT ",
        "INSERT ",
        "UPDATE ",
        "DELETE ",
        "CREATE ",
        "ALTER ",
        "DROP ",
        "echo ",
        "export ",
        "sub ",
        "public ",
        "private ",
        "protected ",
        "template ",
        "namespace ",
        "module ",
        "global ",
        "section ",
        "program ",
        "IDENTIFICATION DIVISION",
        "PROCEDURE DIVISION",
        "ENVIRONMENT DIVISION",
        "DATA DIVISION",
        "cmake_minimum_required",
        "\\documentclass",
        "\\begin",
        "terraform ",
        "syntax =",
        "contract ",
        "resource ",
    ];

    for line in trimmed.lines() {
        let l = line.trim();
        if CODE_LINE_PREFIXES.iter().any(|&p| l.starts_with(p)) {
            return false;
        }
        if l.starts_with("//") || l.starts_with("/*") || l.starts_with("(*") || l.starts_with("#!")
        {
            return false;
        }
    }

    // 3. Sentence endings, contractions, or Latin lorem ipsum vocabulary
    let has_sentence_end = trimmed.lines().any(|l| {
        let trimmed_line = l.trim_end();
        trimmed_line.ends_with('.')
            || trimmed_line.ends_with('?')
            || trimmed_line.ends_with('!')
            || trimmed_line.ends_with(".\"")
            || trimmed_line.ends_with("?\"")
            || trimmed_line.ends_with("!\"")
    });

    let has_contraction = trimmed.contains('\'') || trimmed.contains('’');

    const LOREM_WORDS: &[&str] = &[
        "lorem",
        "ipsum",
        "dolor",
        "sit",
        "amet",
        "consectetur",
        "adipiscing",
        "elit",
        "sed",
        "eiusmod",
        "tempor",
        "incididunt",
        "labore",
        "dolore",
        "magna",
        "aliqua",
    ];
    let lower = trimmed.to_ascii_lowercase();
    let has_lorem = LOREM_WORDS.iter().any(|&w| lower.contains(w));

    if has_sentence_end || has_contraction || has_lorem {
        return true;
    }

    // Plain text without any function call '(' or assignment '='
    let words_count = trimmed.split_whitespace().count();
    if words_count >= 1 && !trimmed.contains('(') && !trimmed.contains('=') {
        return true;
    }

    false
}

/// Maps standard file extensions to syntect language identifiers
pub fn map_extension_to_language(ext: &str) -> Option<&'static str> {
    match ext.to_lowercase().as_str() {
        "txt" | "text" | "log" => Some("txt"),
        "rs" => Some("rs"),
        "py" | "pyw" | "python" => Some("py"),
        "c" | "h" => Some("c"),
        "cpp" | "cc" | "cxx" | "hpp" | "hxx" | "c++" => Some("cpp"),
        "java" => Some("java"),
        "sh" | "bash" | "zsh" | "ksh" => Some("sh"),
        "js" | "mjs" | "cjs" | "jsx" => Some("js"),
        "ts" | "tsx" | "mts" | "cts" => Some("ts"),
        "json" => Some("json"),
        "yaml" | "yml" => Some("yaml"),
        "toml" => Some("toml"),
        "kdl" => Some("kdl"),
        "ini" | "cfg" | "conf" | "env" | "service" | "desktop" | "properties" | "editorconfig"
        | "gitconfig" | "gitmodules" => Some("ini"),
        "html" | "htm" | "xhtml" => Some("html"),
        "css" | "scss" | "sass" | "less" => Some("css"),
        "md" | "markdown" => Some("md"),
        "sql" => Some("sql"),
        "xml" | "svg" | "xsd" => Some("xml"),
        "go" => Some("go"),
        "rb" | "ruby" => Some("rb"),
        "php" => Some("php"),
        "lua" => Some("lua"),
        "swift" => Some("swift"),
        "diff" | "patch" => Some("diff"),
        "cs" | "csx" => Some("cs"),
        "kt" | "kts" => Some("kt"),
        "dart" => Some("dart"),
        "scala" | "sc" | "sbt" => Some("scala"),
        "pl" | "pm" | "t" => Some("pl"),
        "r" | "rprofile" => Some("r"),
        "jl" => Some("jl"),
        "ps1" | "psm1" => Some("ps1"),
        "bat" | "cmd" => Some("bat"),
        "ex" | "exs" => Some("ex"),
        "erl" | "hrl" => Some("erl"),
        "clj" | "cljs" | "cljc" | "edn" => Some("clj"),
        "hs" | "lhs" => Some("hs"),
        "zig" => Some("zig"),
        "groovy" | "gvy" | "gradle" => Some("groovy"),
        "d" | "di" => Some("d"),
        "asm" | "s" => Some("asm"),
        "nim" => Some("nim"),
        "ll" => Some("llvm"),
        "red" | "reds" => Some("red"),
        "odin" => Some("odin"),
        "coffee" => Some("coffee"),
        "raku" | "rakumod" => Some("raku"),
        "hx" => Some("hx"),
        "cr" => Some("cr"),
        "noul" => Some("noul"),
        "wat" | "wasm" => Some("wat"),
        "elm" => Some("elm"),
        "re" | "rei" => Some("re"),
        "purs" => Some("purs"),
        "hack" => Some("hack"),
        "fth" | "4th" => Some("forth"),
        "cob" | "cbl" => Some("cobol"),
        "f" | "for" | "f90" | "f95" => Some("fortran"),
        "pas" | "pp" | "p" | "dpr" => Some("pascal"),
        "bas" | "bi" => Some("vb"),
        "m" => Some("objc"),
        "mm" => Some("objc"),
        "matlab" => Some("matlab"),
        "vb" | "vbs" => Some("vb"),
        "apl" => Some("apl"),
        "moon" => Some("moon"),
        "ahk" | "ah2" => Some("ahk"),
        "cy" => Some("cy"),
        "wren" => Some("wren"),
        "tcl" => Some("tcl"),
        "fish" => Some("fish"),
        "nu" => Some("nu"),
        "xsh" => Some("xsh"),
        "elv" => Some("elv"),
        "tcsh" | "csh" => Some("csh"),
        "rkt" | "scm" | "ss" => Some("scm"),
        "lisp" | "lsp" | "cl" => Some("lisp"),
        "el" => Some("lisp"),
        "janet" => Some("janet"),
        "u" => Some("unison"),
        "roc" => Some("roc"),
        "kk" => Some("koka"),
        "fut" => Some("futhark"),
        "ml" | "mli" => Some("ml"),
        "fs" | "fsi" | "fsx" => Some("fs"),
        "idr" => Some("idr"),
        "agda" => Some("agda"),
        "lean" => Some("lean"),
        "vult" => Some("vult"),
        "vhd" | "vhdl" => Some("vhdl"),
        "scallop" => Some("scallop"),
        "v" => Some("verilog"),
        "wl" | "wls" => Some("wolfram"),
        "sol" => Some("sol"),
        "move" => Some("move"),
        "proto" => Some("proto"),
        "thrift" => Some("thrift"),
        "graphql" | "gql" => Some("graphql"),
        "tf" | "hcl" | "tfvars" => Some("tf"),
        "nix" => Some("nix"),
        "bzl" | "bazel" => Some("bazel"),
        "cmake" => Some("cmake"),
        "vim" | "vimrc" => Some("vim"),
        "awk" => Some("awk"),
        "glsl" | "vert" | "frag" | "geom" | "comp" => Some("glsl"),
        "hlsl" | "fx" | "fxh" => Some("hlsl"),
        "metal" => Some("metal"),
        "tex" | "ltx" | "sty" | "cls" | "bib" => Some("tex"),
        "bf" => Some("bf"),
        _ => None,
    }
}

/// Returns a human-friendly display name for a language identifier
pub fn language_display_name(lang: &str) -> &'static str {
    match lang {
        "rs" => "Rust",
        "py" => "Python",
        "c" => "C",
        "cpp" => "C++",
        "java" => "Java",
        "sh" => "Bash",
        "js" => "JavaScript",
        "ts" => "TypeScript",
        "json" => "JSON",
        "yaml" => "YAML",
        "toml" => "TOML",
        "kdl" => "KDL",
        "ini" => "Configuration",
        "html" => "HTML",
        "css" => "CSS",
        "md" => "Markdown",
        "sql" => "SQL",
        "xml" => "XML",
        "go" => "Go",
        "rb" => "Ruby",
        "php" => "PHP",
        "lua" => "Lua",
        "swift" => "Swift",
        "diff" => "Diff",
        "cs" => "C#",
        "kt" => "Kotlin",
        "dart" => "Dart",
        "scala" => "Scala",
        "pl" => "Perl",
        "r" => "R",
        "jl" => "Julia",
        "ps1" => "PowerShell",
        "bat" => "Batch",
        "ex" => "Elixir",
        "erl" => "Erlang",
        "clj" => "Clojure",
        "hs" => "Haskell",
        "zig" => "Zig",
        "groovy" => "Groovy",
        "d" => "D",
        "asm" => "Assembly",
        "nim" => "Nim",
        "llvm" => "LLVM IR",
        "red" => "Red",
        "odin" => "Odin",
        "coffee" => "CoffeeScript",
        "raku" => "Raku",
        "hx" => "Haxe",
        "cr" => "Crystal",
        "noul" => "Noulith",
        "wat" => "WebAssembly",
        "elm" => "Elm",
        "re" => "ReScript",
        "purs" => "PureScript",
        "hack" => "Hack",
        "forth" => "Forth",
        "cobol" => "COBOL",
        "fortran" => "Fortran",
        "pascal" => "Pascal",
        "objc" => "Objective-C",
        "matlab" => "MATLAB",
        "vb" => "Visual Basic",
        "apl" => "APL",
        "moon" => "MoonScript",
        "ahk" => "AutoHotkey",
        "cy" => "Cyber",
        "wren" => "Wren",
        "tcl" => "Tcl",
        "fish" => "Fish",
        "nu" => "Nushell",
        "xsh" => "Xonsh",
        "elv" => "Elvish",
        "csh" => "C Shell",
        "scm" => "Scheme",
        "lisp" => "Lisp",
        "janet" => "Janet",
        "unison" => "Unison",
        "roc" => "Roc",
        "koka" => "Koka",
        "futhark" => "Futhark",
        "ml" => "OCaml",
        "fs" => "F#",
        "idr" => "Idris",
        "agda" => "Agda",
        "lean" => "Lean",
        "vult" => "Vult",
        "vhdl" => "VHDL",
        "scallop" => "Scallop",
        "verilog" => "Verilog",
        "wolfram" => "Wolfram",
        "sol" => "Solidity",
        "move" => "Move",
        "proto" => "Protobuf",
        "thrift" => "Thrift",
        "graphql" => "GraphQL",
        "tf" => "Terraform / HCL",
        "nix" => "Nix",
        "bazel" => "Bazel",
        "cmake" => "CMake",
        "vim" => "Vim script",
        "awk" => "AWK",
        "glsl" => "GLSL",
        "hlsl" => "HLSL",
        "metal" => "Metal",
        "tex" => "TeX / LaTeX",
        "bf" => "Brainfuck",
        _ => "Text",
    }
}

/// Bundled Sublime Syntax definition for configuration formats (TOML, INI, KDL, etc.).
pub const CONFIG_SUBLIME_SYNTAX: &str = include_str!("syntax/config.sublime-syntax");

/// Bundled Sublime Syntax definition for TypeScript and TSX formats.
pub const TYPESCRIPT_SUBLIME_SYNTAX: &str = include_str!("syntax/typescript.sublime-syntax");

/// Maps a language identifier to a compatible fallback language in syntect's default syntax set.
pub fn syntect_fallback(lang: &str) -> &'static str {
    match lang {
        "ts" | "tsx" | "mts" | "cts" => "js",
        "toml" | "kdl" | "ini" | "cfg" | "conf" | "env" => "yaml",
        "objc" | "mm" => "m",
        "swift" => "rs",
        "bazel" | "bzl" => "py",
        "awk" => "c",
        "kt" | "kts" | "dart" | "hack" => "java",
        "cs" | "csx" => "cs",
        "zig" | "odin" | "vhd" | "vhdl" | "verilog" | "asm" | "llvm" | "vult" | "scallop" => "c",
        "nim" | "jl" | "cy" | "noul" | "wren" => "py",
        "cr" | "moon" => "rb",
        "ex" | "exs" => "rb",
        "fs" | "fsi" | "fsx" => "ml",
        "rkt" | "scm" | "ss" | "janet" | "lisp" => "lisp",
        "elm" | "purs" | "idr" | "agda" | "lean" | "unison" | "roc" | "koka" | "futhark" => "hs",
        "re" => "ml",
        "glsl" | "vert" | "frag" | "hlsl" | "metal" | "proto" | "thrift" | "sol" | "move" => "cpp",
        "tf" | "hcl" | "nix" => "yaml",
        "cmake" => "make",
        "ps1" | "psm1" | "nu" | "xsh" | "elv" | "tcsh" | "csh" | "fish" => "sh",
        "graphql" | "gql" => "js",
        "coffee" => "js",
        "wat" => "lisp",
        "vb" | "vbs" => "pas",
        "f" | "for" | "f90" | "f95" | "fortran" => "pas",
        "pascal" => "pas",
        "tex" => "tex",
        "raku" => "pl",
        "hx" => "java",
        "red" => "rb",
        "ahk" => "bat",
        "tcl" => "tcl",
        "d" => "d",
        "cobol" | "forth" | "apl" | "bf" | "wolfram" | "vim" => "txt",
        _ => "txt",
    }
}

/// Checks if a file path has an extension associated with plain text, code, or markup.
pub fn is_code_file(path: &Path) -> bool {
    path.extension()
        .and_then(|s| s.to_str())
        .and_then(map_extension_to_language)
        .is_some()
}

const CODE_PREFIXES: &[&str] = &[
    "#include",
    "#define",
    "#pragma",
    "#ifdef",
    "#ifndef",
    "import ",
    "from ",
    "package ",
    "use std::",
    "pub fn ",
    "int main(",
    "void main(",
];

fn is_code_indicator(l: &str) -> bool {
    if CODE_PREFIXES.iter().any(|p| l.starts_with(p)) {
        return true;
    }
    if l.starts_with("def ") && l.ends_with(':') {
        return true;
    }
    if l.starts_with("class ") && (l.ends_with(':') || l.ends_with('{')) {
        return true;
    }
    if l.starts_with("fn ") && l.contains('(') {
        return true;
    }
    false
}

fn is_heading_line(l: &str) -> bool {
    let offset = if l.starts_with("### ") {
        Some(4)
    } else if l.starts_with("## ") {
        Some(3)
    } else if l.starts_with("# ") {
        Some(2)
    } else {
        None
    };

    offset
        .and_then(|idx| l.chars().nth(idx))
        .map(|c| c.is_alphanumeric())
        .unwrap_or(false)
}

fn is_list_line(l: &str) -> bool {
    l.len() > 3 && (l.starts_with("- ") || l.starts_with("* ") || l.starts_with("+ "))
}

fn is_link_line(l: &str) -> bool {
    l.contains("](") && l.contains('[') && l.contains(')')
}

#[derive(Default)]
struct MarkdownSignalCounter {
    fence_count: usize,
    heading_count: usize,
    list_count: usize,
    link_count: usize,
}

impl MarkdownSignalCounter {
    fn is_markdown(&self) -> bool {
        if self.fence_count >= 2 {
            return true;
        }
        if self.heading_count >= 1
            && (self.list_count >= 1 || self.link_count >= 1 || self.fence_count >= 1)
        {
            return true;
        }
        self.heading_count >= 2 && self.list_count >= 1
    }

    fn record_line(&mut self, line: &str) {
        if is_heading_line(line) {
            self.heading_count += 1;
        } else if is_list_line(line) {
            self.list_count += 1;
        } else if is_link_line(line) {
            self.link_count += 1;
        }
    }
}

/// Returns true if the text exhibits strong Markdown structural signals and is not code.
pub fn detect_markdown_text(text: &str) -> bool {
    let trimmed = text.trim();
    if trimmed.is_empty() || trimmed.starts_with("#!") {
        return false;
    }

    let mut in_fence = false;
    let mut signals = MarkdownSignalCounter::default();

    for line in trimmed.lines() {
        let l = line.trim();
        if l.starts_with("```") {
            in_fence = !in_fence;
            signals.fence_count += 1;
            continue;
        }

        if in_fence {
            continue;
        }

        if is_code_indicator(l) {
            return false;
        }

        signals.record_line(l);
    }

    signals.is_markdown()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extension_mapping() {
        assert_eq!(detect_language("", Some("rs")), "rs");
        assert_eq!(detect_language("", Some(".py")), "py");
        assert_eq!(detect_language("", Some("cpp")), "cpp");
        assert_eq!(detect_language("", Some("java")), "java");
        assert_eq!(detect_language("", Some("sh")), "sh");
        assert_eq!(detect_language("", Some("json")), "json");
        assert_eq!(detect_language("fn main() {}", Some(".TXT")), "txt");
        assert_eq!(detect_language("fn main() {}", Some("text")), "txt");
        assert!(is_code_file(Path::new("notes.txt")));
        assert!(is_code_file(Path::new("notes.TXT")));
        assert!(is_code_file(Path::new("notes.text")));
        assert!(!is_code_file(Path::new("photo.png")));
    }

    #[test]
    fn test_shebang_detection() {
        assert_eq!(
            detect_language("#!/usr/bin/env python3\nprint('hello')", None),
            "py"
        );
        assert_eq!(detect_language("#!/bin/bash\necho hello", None), "sh");
        assert_eq!(
            detect_language("#!/usr/bin/env node\nconsole.log(1)", None),
            "js"
        );
    }

    #[test]
    fn test_rust_heuristic() {
        let code = "pub fn calculate_sum(a: i32, b: i32) -> i32 {\n    let mut x = a;\n    println!(\"{}\", x);\n    x + b\n}";
        assert_eq!(detect_language(code, None), "rs");
    }

    #[test]
    fn test_python_heuristic() {
        let code = "def fetch_records(client, query):\n    results = client.execute(query)\n    for r in results:\n        print(r)\n    return results";
        assert_eq!(detect_language(code, None), "py");
    }

    #[test]
    fn test_c_heuristic() {
        let code = "#include <stdio.h>\n\nint main(int argc, char** argv) {\n    printf(\"Hello, %s\\n\", argv[1]);\n    return 0;\n}";
        assert_eq!(detect_language(code, None), "c");
    }

    #[test]
    fn test_cpp_heuristic() {
        let code = "#include <iostream>\n#include <vector>\n\nint main() {\n    std::vector<int> nums = {1, 2, 3};\n    std::cout << nums.size() << std::endl;\n    return 0;\n}";
        assert_eq!(detect_language(code, None), "cpp");
    }

    #[test]
    fn test_java_heuristic() {
        let code = "package com.example;\n\npublic class Application {\n    public static void main(String[] args) {\n        System.out.println(\"Running...\");\n    }\n}";
        assert_eq!(detect_language(code, None), "java");
    }

    #[test]
    fn test_bash_heuristic() {
        let code = "export APP_ENV=production\nif [ -f /tmp/lock ]; then\n    echo \"Locked\"\n    exit 1\nfi\necho \"Starting...\"\nchmod +x ./bin/run";
        assert_eq!(detect_language(code, None), "sh");
    }

    #[test]
    fn test_json_and_structural() {
        assert_eq!(
            detect_language("{\"name\": \"test\", \"version\": 1}", None),
            "json"
        );
        assert_eq!(
            detect_language(
                "<!DOCTYPE html><html><body><h1>Hello</h1></body></html>",
                None
            ),
            "html"
        );
    }

    #[test]
    fn test_phase1_languages() {
        // Extension tests
        assert_eq!(detect_language("", Some("cs")), "cs");
        assert_eq!(detect_language("", Some("kt")), "kt");
        assert_eq!(detect_language("", Some("dart")), "dart");
        assert_eq!(detect_language("", Some("scala")), "scala");
        assert_eq!(detect_language("", Some("pl")), "pl");
        assert_eq!(detect_language("", Some("r")), "r");
        assert_eq!(detect_language("", Some("jl")), "jl");
        assert_eq!(detect_language("", Some("ps1")), "ps1");
        assert_eq!(detect_language("", Some("bat")), "bat");
        assert_eq!(detect_language("", Some("ex")), "ex");
        assert_eq!(detect_language("", Some("erl")), "erl");
        assert_eq!(detect_language("", Some("clj")), "clj");
        assert_eq!(detect_language("", Some("hs")), "hs");
        assert_eq!(detect_language("", Some("zig")), "zig");
        assert_eq!(detect_language("", Some("groovy")), "groovy");
        assert_eq!(detect_language("", Some("d")), "d");

        // Shebang tests
        assert_eq!(
            detect_language("#!/usr/bin/env perl\nuse strict;", None),
            "pl"
        );
        assert_eq!(
            detect_language("#!/usr/bin/env elixir\nIO.puts(1)", None),
            "ex"
        );
        assert_eq!(
            detect_language("#!/usr/bin/env julia\nprintln(1)", None),
            "jl"
        );
        assert_eq!(
            detect_language("#!/usr/bin/pwsh\nWrite-Host 1", None),
            "ps1"
        );

        // Snippet / Heuristic tests
        assert_eq!(
            detect_language(
                "using System;\nnamespace App {\n    class Program {\n        static void Main() { Console.WriteLine(\"hi\"); }\n    }\n}",
                None
            ),
            "cs"
        );
        assert_eq!(
            detect_language(
                "fun main() {\n    val message = \"hello\"\n    println(message)\n}",
                None
            ),
            "kt"
        );
        assert_eq!(
            detect_language(
                "import 'package:flutter/material.dart';\nclass MyWidget extends StatelessWidget {\n    Widget build(BuildContext context) {}\n}",
                None
            ),
            "dart"
        );
        assert_eq!(
            detect_language(
                "defmodule Sample.Math do\n  def sum(a, b) do\n    a + b\n  end\nend",
                None
            ),
            "ex"
        );
        assert_eq!(
            detect_language(
                "-module(hello_world).\n-export([start/0]).\nstart() -> io:format(\"Hello\").",
                None
            ),
            "erl"
        );
        assert_eq!(
            detect_language(
                "(ns my.project\n  (:require [clojure.string :as str]))\n(defn greet [name]\n  (println name))",
                None
            ),
            "clj"
        );
        assert_eq!(
            detect_language(
                "module Main where\nimport qualified Data.Map as Map\nmain :: IO ()\nmain = putStrLn \"hello\"",
                None
            ),
            "hs"
        );
        assert_eq!(
            detect_language(
                "const std = @import(\"std\");\npub fn main() !void {\n    std.debug.print(\"hello\\n\", .{});\n}",
                None
            ),
            "zig"
        );
        assert_eq!(
            detect_language(
                "@echo off\nsetlocal enabledelayedexpansion\necho Running script...\ngoto :end\n:end",
                None
            ),
            "bat"
        );
        assert_eq!(
            detect_language(
                "import std.stdio;\nvoid main() {\n    writeln(\"Hello, D!\");\n}",
                None
            ),
            "d"
        );

        // Popularity preference: ambiguous simple snippets prefer Python/JS over obscure languages
        assert_eq!(detect_language("print(\"hello world\")", None), "py");
    }

    #[test]
    fn test_additional_phases_languages() {
        // Extension tests
        assert_eq!(detect_language("", Some("sol")), "sol");
        assert_eq!(detect_language("", Some("glsl")), "glsl");
        assert_eq!(detect_language("", Some("cmake")), "cmake");
        assert_eq!(detect_language("", Some("proto")), "proto");
        assert_eq!(detect_language("", Some("graphql")), "graphql");
        assert_eq!(detect_language("", Some("tf")), "tf");
        assert_eq!(detect_language("", Some("nix")), "nix");
        assert_eq!(detect_language("", Some("tex")), "tex");
        assert_eq!(detect_language("", Some("coffee")), "coffee");
        assert_eq!(detect_language("", Some("asm")), "asm");
        assert_eq!(detect_language("", Some("f90")), "fortran");
        assert_eq!(detect_language("", Some("pas")), "pascal");
        assert_eq!(detect_language("", Some("ml")), "ml");
        assert_eq!(detect_language("", Some("fs")), "fs");
        assert_eq!(detect_language("", Some("scm")), "scm");
        assert_eq!(detect_language("", Some("nim")), "nim");
        assert_eq!(detect_language("", Some("odin")), "odin");
        assert_eq!(detect_language("", Some("cr")), "cr");
        assert_eq!(detect_language("", Some("cbl")), "cobol");
        assert_eq!(detect_language("", Some("fth")), "forth");
        assert_eq!(detect_language("", Some("vb")), "vb");
        assert_eq!(detect_language("", Some("m")), "objc");
        assert_eq!(detect_language("", Some("matlab")), "matlab");
        assert_eq!(detect_language("", Some("bf")), "bf");
        assert_eq!(detect_language("", Some("wat")), "wat");
        assert_eq!(detect_language("", Some("elm")), "elm");
        assert_eq!(detect_language("", Some("purs")), "purs");
        assert_eq!(detect_language("", Some("re")), "re");
        assert_eq!(detect_language("", Some("hx")), "hx");
        assert_eq!(detect_language("", Some("raku")), "raku");
        assert_eq!(detect_language("", Some("red")), "red");
        assert_eq!(detect_language("", Some("vhd")), "vhdl");
        assert_eq!(detect_language("", Some("v")), "verilog");
        assert_eq!(detect_language("", Some("apl")), "apl");
        assert_eq!(detect_language("", Some("ahk")), "ahk");
        assert_eq!(detect_language("", Some("moon")), "moon");
        assert_eq!(detect_language("", Some("wren")), "wren");
        assert_eq!(detect_language("", Some("nu")), "nu");
        assert_eq!(detect_language("", Some("fish")), "fish");

        // Snippet classification tests
        assert_eq!(
            detect_language(
                "pragma solidity ^0.8.0;\ncontract MyToken { mapping(address => uint256) public balances; }",
                None
            ),
            "sol"
        );
        assert_eq!(
            detect_language(
                "cmake_minimum_required(VERSION 3.10)\nproject(MyProject)\nadd_executable(app main.cpp)",
                None
            ),
            "cmake"
        );
        assert_eq!(
            detect_language(
                "syntax = \"proto3\";\nmessage UserRequest { string id = 1; int32 count = 2; }",
                None
            ),
            "proto"
        );
        assert_eq!(
            detect_language(
                "terraform {\n  required_providers {\n    aws = {}\n  }\n}\nresource \"aws_s3_bucket\" \"b\" {}",
                None
            ),
            "tf"
        );
        assert_eq!(
            detect_language(
                "\\documentclass{article}\n\\begin{document}\nHello LaTeX\n\\end{document}",
                None
            ),
            "tex"
        );
        assert_eq!(
            detect_language(
                "section .data\nmsg db 'Hello', 0\nsection .text\nglobal _start\n_start:\n  mov eax, 1\n  ret",
                None
            ),
            "asm"
        );
        assert_eq!(
            detect_language(
                "program Hello\n  implicit none\n  print *, 'Hello, Fortran!'\nend program Hello",
                None
            ),
            "fortran"
        );
        assert_eq!(
            detect_language(
                "program HelloWorld;\nbegin\n  writeln('Hello, Pascal!');\nend.",
                None
            ),
            "pascal"
        );
        assert_eq!(
            detect_language(
                "IDENTIFICATION DIVISION.\nPROGRAM-ID. HELLO.\nPROCEDURE DIVISION.\n    DISPLAY 'Hello, COBOL!'.\n    STOP RUN.",
                None
            ),
            "cobol"
        );

        // Plain text without code structures should stay txt
        assert_eq!(detect_language("Plain notes.\n", None), "txt");
        assert_eq!(
            detect_language("A walk through the garden.\nAnother lovely day.\n", None),
            "txt"
        );
    }

    #[test]
    fn test_user_snippet() {
        let text_unicode = "And maybe that’s possible still, maybe over the course of a decade OpGuides will still end up as that. But I don’t think that’s even what I want anymore. I don’t want this to be Vega’s Opinionated Big Ass Book To Give You An All-In-One Education, both because VegaOpBABTGYAAIOEdu is far less catchy, and because I don’t even think it’s possible. The more I write on this the more I value input of others and other resources I find, and the more happy I am that I called this project Opinionated Guides.";
        assert_eq!(detect_language(text_unicode, None), "txt");

        let text_ascii = "And maybe that's possible still, maybe over the course of a decade OpGuides will still end up as that. But I don't think that's even what I want anymore. I don't want this to be Vega's Opinionated Big Ass Book To Give You An All-In-One Education, both because VegaOpBABTGYAAIOEdu is far less catchy, and because I don't even think it's possible. The more I write on this the more I value input of others and other resources I find, and the more happy I am that I called this project Opinionated Guides.";
        assert_eq!(detect_language(text_ascii, None), "txt");
    }

    #[test]
    fn test_plain_prose_and_lorem_ipsum_stay_plain_text() {
        let cases = [
            "Lorem ipsum dolor sit amet, consectetur adipiscing elit, sed do eiusmod tempor incididunt ut labore et dolore magna aliqua.",
            "Lorem ipsum dolor sit amet, consectetur adipiscing elit, sed do eiusmod tempor incididunt ut labore et dolore magna aliqua. Ut enim ad minim veniam, quis nostrud exercitation ullamco laboris nisi ut aliquip ex ea commodo consequat. Duis aute irure dolor in reprehenderit in voluptate velit esse cillum dolore eu fugiat nulla pariatur. Excepteur sint occaecat cupidatat non proident, sunt in culpa qui officia deserunt mollit anim id est laborum.",
            "Lorem ipsum dolor sit amet, but it's placeholder text.",
            "Here's some text with an apostrophe and quotes: 'lorem ipsum'.",
            "'Lorem ipsum dolor sit amet'",
            "Sed ut perspiciatis unde omnis iste natus error sit voluptatem accusantium doloremque laudantium, totam rem aperiam, eaque ipsa quae ab illo inventore veritatis et quasi architecto beatae vitae dicta sunt explicabo.",
            "That's one small step for man, one giant leap for mankind.",
            "Yes, let's do this.",
            "This is a test note, it's plain text.",
            "Just a quick note, don't forget milk.",
            "Don't worry, be happy.",
            "It's raining outside today.",
            "User's manual and reference guide.",
            "What's the meaning of life?",
            "Let's write some plain English text with an apostrophe.",
            "There's no place like home.",
            "Plain notes.\n",
            "A walk through the garden.\nAnother lovely day.\n",
        ];

        for text in cases {
            assert_eq!(
                detect_language(text, None),
                "txt",
                "Text should be classified as plain text ('txt'), but got: {}",
                detect_language(text, None)
            );
        }
    }

    #[test]
    fn test_syntect_fallback_and_bundled_syntax() {
        assert!(!CONFIG_SUBLIME_SYNTAX.is_empty());
        assert!(!TYPESCRIPT_SUBLIME_SYNTAX.is_empty());

        assert_eq!(syntect_fallback("ts"), "js");
        assert_eq!(syntect_fallback("tsx"), "js");
        assert_eq!(syntect_fallback("toml"), "yaml");
        assert_eq!(syntect_fallback("ini"), "yaml");
        assert_eq!(syntect_fallback("objc"), "m");
        assert_eq!(syntect_fallback("swift"), "rs");
        assert_eq!(syntect_fallback("bazel"), "py");
        assert_eq!(syntect_fallback("kt"), "java");
        assert_eq!(syntect_fallback("zig"), "c");
    }
}
