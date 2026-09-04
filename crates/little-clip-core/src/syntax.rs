use std::path::Path;

/// Detects the programming or markup language from an optional file extension hint
/// and content heuristics, prioritizing common languages: Python, C/C++, Rust, Java, Bash, etc.
pub fn detect_language(content: &str, ext_hint: Option<&str>) -> &'static str {
    // 1. If explicit extension hint is provided, normalize and map it
    if let Some(ext) = ext_hint {
        let clean = ext.trim().trim_start_matches('.').to_lowercase();
        if !clean.is_empty() && clean != "txt" && clean != "text" {
            if let Some(lang) = map_extension_to_language(&clean) {
                return lang;
            }
        }
    }

    let trimmed = content.trim();
    if trimmed.is_empty() {
        return "txt";
    }

    // 2. Shebang line check (e.g. #!/usr/bin/env python3, #!/bin/bash)
    if trimmed.starts_with("#!") {
        if let Some(first_line) = trimmed.lines().next() {
            let fl = first_line.to_lowercase();
            if fl.contains("python") {
                return "py";
            }
            if fl.contains("bash") || fl.contains("/sh") || fl.contains("zsh") {
                return "sh";
            }
            if fl.contains("node") || fl.contains("deno") || fl.contains("bun") {
                return "js";
            }
            if fl.contains("ruby") {
                return "rb";
            }
            if fl.contains("perl") {
                return "pl";
            }
        }
    }

    // 3. Fast structural checks
    if (trimmed.starts_with('{') && trimmed.ends_with('}'))
        || (trimmed.starts_with('[') && trimmed.ends_with(']'))
    {
        if serde_json::from_str::<serde_json::Value>(trimmed).is_ok() {
            return "json";
        }
    }

    if trimmed.starts_with("<!DOCTYPE") || trimmed.starts_with("<html") || trimmed.starts_with("<?xml") {
        return "html";
    }

    if trimmed.starts_with("---") && trimmed.contains(": ") {
        return "yaml";
    }

    if (trimmed.starts_with("[package]") || trimmed.starts_with("[dependencies]") || trimmed.starts_with("[workspace]"))
        && trimmed.contains('=')
    {
        return "toml";
    }

    // 4. Scoring heuristics prioritizing Python, C/C++, Rust, Java, Bash
    let mut py_score: u32 = 0;
    let mut rs_score: u32 = 0;
    let mut c_score: u32 = 0;
    let mut cpp_score: u32 = 0;
    let mut java_score: u32 = 0;
    let mut sh_score: u32 = 0;
    let mut js_score: u32 = 0;

    // Line-by-line inspection
    for line in trimmed.lines().take(40) {
        let l = line.trim();

        // Rust patterns
        if l.starts_with("fn ") || l.starts_with("pub fn ") || l.starts_with("pub async fn ") {
            rs_score += 4;
        }
        if l.starts_with("use std::") || l.starts_with("use crate::") || l.starts_with("pub use ") {
            rs_score += 3;
        }
        if l.starts_with("impl ") || l.starts_with("impl<") || l.starts_with("trait ") {
            rs_score += 3;
        }
        if l.starts_with("let mut ") || l.contains("let mut ") {
            rs_score += 2;
        }
        if l.contains("println!(") || l.contains("eprintln!(") || l.contains("vec![") || l.contains("format!(") {
            rs_score += 3;
        }
        if l.contains("match ") && l.ends_with('{') {
            rs_score += 2;
        }
        if l.contains("-> Result<") || l.contains("-> Option<") {
            rs_score += 2;
        }

        // Python patterns
        if l.starts_with("def ") && l.ends_with(':') {
            py_score += 4;
        }
        if l.starts_with("class ") && l.ends_with(':') {
            py_score += 3;
        }
        if (l.starts_with("import ") || l.starts_with("from ")) && !l.ends_with(';') {
            py_score += 3;
        }
        if l.starts_with("if __name__ == ") || l.contains("__init__") {
            py_score += 4;
        }
        if l.starts_with("elif ") || l.starts_with("except ") || l.starts_with("finally:") {
            py_score += 3;
        }
        if l.contains("print(") && !l.ends_with(';') {
            py_score += 2;
        }
        if l.contains("self.") {
            py_score += 2;
        }

        // C / C++ patterns
        if l.starts_with("#include <") || l.starts_with("#include \"") {
            c_score += 4;
            if l.contains("<iostream>") || l.contains("<vector>") || l.contains("<string>") || l.contains("<memory>") {
                cpp_score += 4;
            }
        }
        if l.starts_with("#define ") {
            c_score += 2;
        }
        if l.contains("int main(") || l.contains("void main(") || l.contains("int main (") {
            c_score += 3;
        }
        if l.contains("std::") || l.contains("cout <<") || l.contains("cin >>") || l.contains("nullptr") {
            cpp_score += 4;
        }
        if l.starts_with("template <") || l.starts_with("template<") || l.contains("namespace ") {
            cpp_score += 3;
        }
        if l.contains("printf(") || l.contains("scanf(") || l.contains("malloc(") {
            c_score += 3;
        }

        // Java patterns
        if l.contains("public static void main") || l.contains("System.out.print") || l.contains("System.err.print") {
            java_score += 5;
        }
        if l.starts_with("public class ") || l.starts_with("private class ") || l.starts_with("public interface ") {
            java_score += 4;
        }
        if l.starts_with("package ") && l.ends_with(';') {
            java_score += 3;
        }
        if l.starts_with("import java.") || l.starts_with("import javax.") {
            java_score += 4;
        }
        if l.contains("@Override") || l.contains("throws Exception") {
            java_score += 2;
        }

        // Bash patterns
        if l.starts_with("echo ") || l.starts_with("echo -e ") {
            sh_score += 2;
        }
        if l.starts_with("export ") && l.contains('=') {
            sh_score += 3;
        }
        if l.starts_with("if [") || l.starts_with("if [[") || l.starts_with("elif [") {
            sh_score += 3;
        }
        if l == "fi" || l == "done" || l == "esac" {
            sh_score += 4;
        }
        if l.starts_with("chmod ") || l.starts_with("chown ") || l.starts_with("source ") {
            sh_score += 2;
        }
        if l.contains("${") && l.contains('}') {
            sh_score += 1;
        }

        // JS / TS patterns
        if l.starts_with("const ") || l.starts_with("let ") || l.starts_with("var ") {
            if l.contains(" = ") {
                js_score += 1;
            }
        }
        if l.contains("console.log(") || l.contains("console.error(") {
            js_score += 3;
        }
        if l.starts_with("export default ") || l.starts_with("import React") {
            js_score += 3;
        }
        if l.starts_with("interface ") && l.ends_with('{') {
            js_score += 3;
        }
    }

    // Determine the highest scoring language
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
            return lang;
        }
    }

    // 5. Fallback heuristics for short snippets
    if trimmed.contains("fn ") || trimmed.contains("println!") {
        return "rs";
    }
    if trimmed.contains("def ") || trimmed.contains("print(") {
        return "py";
    }
    if trimmed.contains("#include") || trimmed.contains("printf(") {
        return "c";
    }
    if trimmed.contains("echo ") || trimmed.contains("export ") {
        return "sh";
    }
    if trimmed.contains("public class") || trimmed.contains("System.out.") {
        return "java";
    }

    "txt"
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
