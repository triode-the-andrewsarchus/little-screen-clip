//! Programming and markup language detection heuristics.
//!
//! Classifies plain text and code snippets into language tags (`rs`, `py`, `c`, `cpp`, `java`, `sh`, `json`, etc.)
//! using file extension hints, shebang lines, structural markers, and keyword scoring.

use std::path::Path;

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

    // 4. Scoring heuristics prioritizing Python, C/C++, Rust, Java, Bash, JS/TS
    if let Some(lang) = score_language_heuristics(trimmed) {
        return lang;
    }

    // 5. Fallback heuristics for short snippets
    check_snippet_fallback(trimmed).unwrap_or("txt")
}

fn check_extension_hint(ext_hint: Option<&str>) -> Option<&'static str> {
    let ext = ext_hint?;
    let clean = ext.trim().trim_start_matches('.').to_lowercase();
    if !clean.is_empty() && clean != "txt" && clean != "text" {
        map_extension_to_language(&clean)
    } else {
        None
    }
}

fn check_shebang(trimmed: &str) -> Option<&'static str> {
    if !trimmed.starts_with("#!") {
        return None;
    }
    let first_line = trimmed.lines().next()?.to_lowercase();
    if first_line.contains("python") {
        Some("py")
    } else if first_line.contains("bash") || first_line.contains("/sh") || first_line.contains("zsh") {
        Some("sh")
    } else if first_line.contains("node") || first_line.contains("deno") || first_line.contains("bun") {
        Some("js")
    } else if first_line.contains("ruby") {
        Some("rb")
    } else if first_line.contains("perl") {
        Some("pl")
    } else {
        None
    }
}

fn check_structural_format(trimmed: &str) -> Option<&'static str> {
    if (trimmed.starts_with('{') && trimmed.ends_with('}'))
        || (trimmed.starts_with('[') && trimmed.ends_with(']'))
    {
        if serde_json::from_str::<serde_json::Value>(trimmed).is_ok() {
            return Some("json");
        }
    }

    if trimmed.starts_with("<!DOCTYPE") || trimmed.starts_with("<html") || trimmed.starts_with("<?xml") {
        return Some("html");
    }

    if trimmed.starts_with("---") && trimmed.contains(": ") {
        return Some("yaml");
    }

    if (trimmed.starts_with("[package]") || trimmed.starts_with("[dependencies]") || trimmed.starts_with("[workspace]"))
        && trimmed.contains('=')
    {
        return Some("toml");
    }

    None
}

fn score_language_heuristics(trimmed: &str) -> Option<&'static str> {
    let mut py_score: u32 = 0;
    let mut rs_score: u32 = 0;
    let mut c_score: u32 = 0;
    let mut cpp_score: u32 = 0;
    let mut java_score: u32 = 0;
    let mut sh_score: u32 = 0;
    let mut js_score: u32 = 0;

    for line in trimmed.lines().take(40) {
        let l = line.trim();
        rs_score += score_rust_line(l);
        py_score += score_python_line(l);
        let (c_inc, cpp_inc) = score_c_cpp_line(l);
        c_score += c_inc;
        cpp_score += cpp_inc;
        java_score += score_java_line(l);
        sh_score += score_bash_line(l);
        js_score += score_js_line(l);
    }

    let scores = [
        (py_score, "py"),
        (rs_score, "rs"),
        (cpp_score, "cpp"),
        (c_score, "c"),
        (java_score, "java"),
        (sh_score, "sh"),
        (js_score, "js"),
    ];

    if let Some(&(max_score, lang)) = scores.iter().max_by_key(|(s, _)| *s) {
        if max_score >= 2 {
            return Some(lang);
        }
    }

    None
}

fn score_rust_line(l: &str) -> u32 {
    let mut score = 0;
    if ["fn ", "pub fn ", "pub async fn "].iter().any(|p| l.starts_with(p)) {
        score += 4;
    }
    if ["use std::", "use crate::", "pub use "].iter().any(|p| l.starts_with(p)) {
        score += 3;
    }
    if ["impl ", "impl<", "trait "].iter().any(|p| l.starts_with(p)) {
        score += 3;
    }
    if l.contains("let mut ") {
        score += 2;
    }
    if ["println!(", "eprintln!(", "vec![", "format!("].iter().any(|p| l.contains(p)) {
        score += 3;
    }
    if l.contains("match ") && l.ends_with('{') {
        score += 2;
    }
    if ["-> Result<", "-> Option<"].iter().any(|p| l.contains(p)) {
        score += 2;
    }
    score
}

fn score_python_line(l: &str) -> u32 {
    let mut score = 0;
    if l.starts_with("def ") && l.ends_with(':') {
        score += 4;
    }
    if l.starts_with("class ") && l.ends_with(':') {
        score += 3;
    }
    if ["import ", "from "].iter().any(|p| l.starts_with(p)) && !l.ends_with(';') {
        score += 3;
    }
    if l.starts_with("if __name__ == ") || l.contains("__init__") {
        score += 4;
    }
    if ["elif ", "except ", "finally:"].iter().any(|p| l.starts_with(p)) {
        score += 3;
    }
    if l.contains("print(") && !l.ends_with(';') {
        score += 2;
    }
    if l.contains("self.") {
        score += 2;
    }
    score
}

fn score_c_cpp_line(l: &str) -> (u32, u32) {
    let mut c_score = 0;
    let mut cpp_score = 0;
    if ["#include <", "#include \""].iter().any(|p| l.starts_with(p)) {
        c_score += 4;
        if ["<iostream>", "<vector>", "<string>", "<memory>"].iter().any(|p| l.contains(p)) {
            cpp_score += 4;
        }
    }
    if l.starts_with("#define ") {
        c_score += 2;
    }
    if ["int main(", "void main(", "int main ("].iter().any(|p| l.contains(p)) {
        c_score += 3;
    }
    if ["std::", "cout <<", "cin >>", "nullptr"].iter().any(|p| l.contains(p)) {
        cpp_score += 4;
    }
    if ["template <", "template<", "namespace "].iter().any(|p| l.contains(p)) {
        cpp_score += 3;
    }
    if ["printf(", "scanf(", "malloc("].iter().any(|p| l.contains(p)) {
        c_score += 3;
    }
    (c_score, cpp_score)
}

fn score_java_line(l: &str) -> u32 {
    let mut score = 0;
    if ["public static void main", "System.out.print", "System.err.print"].iter().any(|p| l.contains(p)) {
        score += 5;
    }
    if ["public class ", "private class ", "public interface "].iter().any(|p| l.starts_with(p)) {
        score += 4;
    }
    if l.starts_with("package ") && l.ends_with(';') {
        score += 3;
    }
    if ["import java.", "import javax."].iter().any(|p| l.starts_with(p)) {
        score += 4;
    }
    if l.contains("@Override") || l.contains("throws Exception") {
        score += 2;
    }
    score
}

fn score_bash_line(l: &str) -> u32 {
    let mut score = 0;
    if ["echo ", "echo -e "].iter().any(|p| l.starts_with(p)) {
        score += 2;
    }
    if l.starts_with("export ") && l.contains('=') {
        score += 3;
    }
    if ["if [", "if [[", "elif ["].iter().any(|p| l.starts_with(p)) {
        score += 3;
    }
    if ["fi", "done", "esac"].contains(&l) {
        score += 4;
    }
    if ["chmod ", "chown ", "source "].iter().any(|p| l.starts_with(p)) {
        score += 2;
    }
    if l.contains("${") && l.contains('}') {
        score += 1;
    }
    score
}

fn score_js_line(l: &str) -> u32 {
    let mut score = 0;
    if (l.starts_with("const ") || l.starts_with("let ") || l.starts_with("var ")) && l.contains(" = ") {
        score += 1;
    }
    if l.contains("console.log(") || l.contains("console.error(") {
        score += 3;
    }
    if l.starts_with("export default ") || l.starts_with("import React") {
        score += 3;
    }
    if l.starts_with("interface ") && l.ends_with('{') {
        score += 3;
    }
    score
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
    } else {
        None
    }
}

/// Maps standard file extensions to syntect language identifiers
pub fn map_extension_to_language(ext: &str) -> Option<&'static str> {
    match ext.to_lowercase().as_str() {
        "rs" => Some("rs"),
        "py" | "pyw" | "python" => Some("py"),
        "c" | "h" => Some("c"),
        "cpp" | "cc" | "cxx" | "hpp" | "hxx" | "c++" => Some("cpp"),
        "java" => Some("java"),
        "sh" | "bash" | "zsh" | "ksh" => Some("sh"),
        "js" | "mjs" | "cjs" | "jsx" => Some("js"),
        "ts" | "tsx" => Some("ts"),
        "json" => Some("json"),
        "yaml" | "yml" => Some("yaml"),
        "toml" => Some("toml"),
        "html" | "htm" | "xhtml" => Some("html"),
        "css" | "scss" | "sass" | "less" => Some("css"),
        "md" | "markdown" => Some("md"),
        "sql" => Some("sql"),
        "xml" | "svg" => Some("xml"),
        "go" => Some("go"),
        "rb" | "ruby" => Some("rb"),
        "php" => Some("php"),
        "lua" => Some("lua"),
        "swift" => Some("swift"),
        "diff" | "patch" => Some("diff"),
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
        _ => "Text",
    }
}

/// Checks if a file path has an extension associated with code or markup
pub fn is_code_file(path: &Path) -> bool {
    path.extension()
        .and_then(|s| s.to_str())
        .and_then(map_extension_to_language)
        .is_some()
}

const CODE_PREFIXES: &[&str] = &[
    "#include", "#define", "#pragma", "#ifdef", "#ifndef",
    "import ", "from ", "package ", "use std::", "pub fn ",
    "int main(", "void main(",
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
        if self.heading_count >= 1 && (self.list_count >= 1 || self.link_count >= 1 || self.fence_count >= 1) {
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
    }

    #[test]
    fn test_shebang_detection() {
        assert_eq!(detect_language("#!/usr/bin/env python3\nprint('hello')", None), "py");
        assert_eq!(detect_language("#!/bin/bash\necho hello", None), "sh");
        assert_eq!(detect_language("#!/usr/bin/env node\nconsole.log(1)", None), "js");
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
        assert_eq!(detect_language("{\"name\": \"test\", \"version\": 1}", None), "json");
        assert_eq!(detect_language("<!DOCTYPE html><html><body><h1>Hello</h1></body></html>", None), "html");
    }
}
