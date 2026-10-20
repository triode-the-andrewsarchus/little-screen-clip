#!/usr/bin/env python3
"""
Builds the compact precomputed token frequencies dataset for the syntax classifier
from GitHub Linguist's / Enry's statistical corpus.
"""

import gzip
import json
import math
import os
import re
import urllib.request

URL = "https://raw.githubusercontent.com/go-enry/go-enry/master/data/frequencies.go"

LANG_MAP = {
    "Rust": "rs", "Python": "py", "C": "c", "C++": "cpp", "C#": "cs", "Go": "go",
    "JavaScript": "js", "TypeScript": "ts", "Java": "java", "Kotlin": "kt", "Swift": "swift",
    "Dart": "dart", "Ruby": "rb", "PHP": "php", "Shell": "sh", "Batchfile": "bat",
    "PowerShell": "ps1", "SQL": "sql", "HTML": "html", "CSS": "css", "SCSS": "css",
    "Perl": "pl", "R": "r", "Julia": "jl", "Scala": "scala", "Haskell": "hs",
    "Elixir": "ex", "Erlang": "erl", "Clojure": "clj", "Zig": "zig", "Lua": "lua",
    "D": "d", "Groovy": "groovy", "OCaml": "ml", "Standard ML": "ml", "F#": "fs",
    "Common Lisp": "lisp", "Emacs Lisp": "lisp", "Scheme": "scm", "Racket": "scm",
    "Janet": "janet", "Pascal": "pascal", "Fortran": "fortran", "Assembly": "asm",
    "Nim": "nim", "Crystal": "cr", "Odin": "odin", "Red": "red", "COBOL": "cobol",
    "Forth": "forth", "Visual Basic .NET": "vb", "Visual Basic 6.0": "vb", "APL": "apl",
    "Tcl": "tcl", "AutoHotkey": "ahk", "MoonScript": "moon", "Wren": "wren",
    "CoffeeScript": "coffee", "Raku": "raku", "Elm": "elm", "PureScript": "purs",
    "ReScript": "re", "Idris": "idr", "Agda": "agda", "Lean": "lean", "Futhark": "futhark",
    "Koka": "koka", "Roc": "roc", "VHDL": "vhdl", "Verilog": "verilog",
    "Wolfram Language": "wolfram", "Move": "move", "Protocol Buffer": "proto",
    "Thrift": "thrift", "GraphQL": "graphql", "HCL": "tf", "Nix": "nix",
    "CMake": "cmake", "Vim Script": "vim", "AWK": "awk", "GLSL": "glsl", "HLSL": "hlsl",
    "Metal": "metal", "TeX": "tex", "Brainfuck": "bf", "WebAssembly": "wat",
    "Fish": "fish", "Nushell": "nu", "Tcsh": "csh", "Hack": "hack", "Solidity": "sol"
}

POPULARITY_RANK = {
    "py": 100, "js": 98, "ts": 95, "c": 94, "cpp": 94, "cs": 92,
    "java": 92, "rs": 90, "go": 90, "sh": 88, "sql": 88, "html": 85,
    "css": 85, "rb": 82, "php": 82, "kt": 78, "swift": 78, "dart": 75,
    "ps1": 72, "r": 70, "scala": 70, "pl": 68, "lua": 68, "hs": 65,
    "ex": 65, "clj": 65, "zig": 62, "jl": 60, "groovy": 60, "bat": 60,
    "d": 55, "erl": 55, "ml": 52, "cmake": 52, "tex": 52, "sol": 50,
}

LANGUAGE_KEYWORDS = {
    "sh": ["echo", "export", "if", "then", "fi", "elif", "else", "for", "do", "done", "case", "esac", "chmod", "chown", "source", "set", "exit", "local", "function", "while", "until"],
    "clj": ["defn", "ns", "def", "let", "fn", "println", "require", "assoc", "dissoc", "conj", "vec", "filter", "map", "reduce", "recur", "loop", "when", "cond"],
    "kt": ["fun", "val", "var", "println", "suspend", "companion", "object", "data", "sealed", "package", "override", "when"],
    "dart": ["void", "main", "flutter", "Widget", "BuildContext", "StatefulWidget", "StatelessWidget", "setState", "final", "const", "async", "await", "Future"],
    "cs": ["namespace", "using", "Console", "WriteLine", "class", "public", "private", "protected", "static", "void", "async", "Task", "get", "set", "var", "new"],
    "rs": ["fn", "pub", "let", "mut", "println!", "eprintln!", "impl", "trait", "struct", "enum", "match", "use", "mod", "crate", "async", "await", "Result", "Option"],
    "py": ["def", "class", "import", "from", "return", "if", "elif", "else", "for", "in", "while", "try", "except", "finally", "with", "as", "lambda", "yield", "self", "print"],
    "c": ["include", "stdio", "printf", "int", "char", "void", "return", "sizeof", "malloc", "free", "struct", "typedef"],
    "cpp": ["include", "iostream", "std", "cout", "cin", "endl", "vector", "string", "template", "typename", "class", "namespace", "nullptr", "auto", "virtual"],
    "java": ["public", "static", "void", "main", "String", "System", "out", "println", "class", "interface", "extends", "implements", "package", "import", "new", "this", "super", "try", "catch", "throw", "throws"],
    "js": ["const", "let", "var", "function", "return", "console", "log", "export", "default", "import", "from", "async", "await", "new", "this", "document", "window"],
    "ts": ["interface", "type", "namespace", "declare", "readonly", "as", "implements", "public", "private", "protected", "keyof", "typeof", "Promise"],
    "go": ["package", "import", "func", "return", "var", "const", "type", "struct", "interface", "chan", "go", "select", "defer", "fmt", "Println", "nil", "make", "range"],
    "rb": ["def", "end", "class", "module", "require", "puts", "attr_accessor", "attr_reader", "do", "yield", "self", "nil", "begin", "rescue", "ensure"],
    "php": ["echo", "function", "class", "public", "private", "protected", "return", "namespace", "use", "new", "this", "array", "null", "isset"],
    "pl": ["my", "our", "sub", "use", "strict", "warnings", "print", "shift", "chomp", "foreach", "package", "die", "warn", "unless", "until"],
    "r": ["library", "data.frame", "ggplot", "c", "list", "matrix", "function", "return", "print", "summary", "read.csv", "install.packages"],
    "jl": ["function", "end", "using", "module", "println", "Vector", "Array", "Float64", "Int64", "struct", "macro", "export"],
    "scala": ["object", "def", "val", "var", "extends", "trait", "case", "class", "match", "println", "implicit", "sealed", "yield", "override"],
    "hs": ["module", "where", "import", "qualified", "let", "in", "data", "type", "newtype", "class", "instance", "putStrLn", "IO", "Just", "Nothing"],
    "ex": ["defmodule", "def", "defp", "do", "end", "IO", "puts", "inspect", "alias", "import", "require", "use", "case", "cond", "when"],
    "erl": ["-module", "-export", "-record", "io", "format", "receive", "after", "spawn", "case", "of", "end", "fun"],
    "zig": ["@import", "pub", "fn", "main", "!void", "const", "var", "std", "debug", "print", "try", "catch", "unreachable", "comptime"],
    "lua": ["local", "function", "end", "return", "then", "do", "repeat", "until", "print", "require", "nil", "table", "string"],
    "d": ["import", "stdio", "writeln", "writef", "void", "main", "string", "auto", "foreach", "unittest", "immutable", "mixin"],
    "groovy": ["plugins", "dependencies", "repositories", "task", "def", "println", "it", "delegate"],
    "bat": ["@echo", "off", "setlocal", "goto", "rem", "echo", "pause", "exit", "call", "if", "errorlevel"],
    "ps1": ["Write-Host", "Write-Output", "Get-Process", "Get-Item", "Set-Item", "param", "CmdletBinding", "function", "foreach", "mandatory"],
    "sql": ["SELECT", "FROM", "WHERE", "INSERT", "INTO", "UPDATE", "DELETE", "CREATE", "TABLE", "DROP", "ALTER", "JOIN", "LEFT", "RIGHT", "GROUP", "BY", "ORDER", "HAVING", "LIMIT"],
    "html": ["<!DOCTYPE", "html", "head", "body", "div", "span", "table", "tr", "td", "script", "style", "link", "meta", "title", "href", "src"],
    "css": ["margin", "padding", "display", "flex", "grid", "background", "color", "font-family", "font-size", "border", "width", "height"],
    "sol": ["pragma", "solidity", "contract", "mapping", "address", "uint256", "payable", "public", "view", "returns", "modifier", "event", "emit"],
    "swift": ["import", "func", "var", "let", "guard", "if", "else", "return", "struct", "class", "enum", "protocol", "extension", "override", "private", "public", "SwiftUI", "View", "print"],
    "nim": ["proc", "import", "echo", "var", "let", "const", "type", "object", "discard", "template", "macro", "when"],
    "cr": ["def", "class", "module", "puts", "require", "end", "struct", "enum", "alias", "include", "extend"],
    "odin": ["package", "import", "proc", "main", "cstring", "fmt", "println", "printf", "struct", "enum", "union", "dynamic"],
    "red": ["Red", "needs", "view", "layout", "button", "text", "field", "do", "either", "print", "probe"],
    "cobol": ["IDENTIFICATION", "DIVISION", "PROGRAM-ID", "ENVIRONMENT", "DATA", "WORKING-STORAGE", "SECTION", "PROCEDURE", "DISPLAY", "STOP", "RUN"],
    "forth": [":", ";", "DUP", "SWAP", "DROP", "ROT", "OVER", "VARIABLE", "CONSTANT", "CREATE", "DOES>", "THEN", "ELSE", "IF"],
    "pascal": ["program", "begin", "end", "var", "procedure", "function", "writeln", "readln", "uses", "type", "const"],
    "fortran": ["program", "end", "subroutine", "function", "implicit", "none", "integer", "real", "print", "write", "format", "do"],
    "asm": ["global", "section", "mov", "push", "pop", "ret", "call", "add", "sub", "jmp", "cmp", "eax", "ebx", "rax", "rsp", "rbp"],
    "wat": ["module", "func", "export", "import", "param", "result", "local", "get", "set", "i32", "i64", "call", "drop"],
    "cmake": ["cmake_minimum_required", "project", "add_executable", "add_library", "target_link_libraries", "set", "find_package", "message"],
    "tex": ["documentclass", "begin", "end", "usepackage", "section", "subsection", "label", "ref", "cite", "textbf", "textit"],
    "vim": ["function", "endfunction", "set", "let", "map", "nmap", "vmap", "autocmd", "call", "execute"],
    "awk": ["BEGIN", "END", "print", "printf", "split", "length", "substr", "getline"],
    "glsl": ["precision", "uniform", "attribute", "varying", "vec2", "vec3", "vec4", "mat4", "sampler2D", "texture2D", "gl_Position", "gl_FragColor", "void", "main"],
    "hlsl": ["cbuffer", "Texture2D", "SamplerState", "float4", "float3", "float2", "matrix", "SV_Position", "SV_Target", "VS_INPUT", "PS_INPUT", "technique"],
    "metal": ["using", "namespace", "metal", "vertex", "fragment", "kernel", "float4", "float3", "float2", "device", "constant", "thread"],
    "proto": ["syntax", "proto3", "proto2", "message", "enum", "service", "rpc", "returns", "repeated", "optional", "package", "import"],
    "graphql": ["query", "mutation", "subscription", "type", "input", "schema", "directive", "scalar", "interface", "union"],
    "tf": ["terraform", "provider", "resource", "data", "variable", "output", "module", "locals"],
    "nix": ["builtins", "import", "inherit", "let", "in", "rec", "with", "fetchFromGitHub", "stdenv", "mkDerivation"],
    "fish": ["function", "end", "set", "echo", "status", "test", "switch", "case"],
    "nu": ["def", "let", "mut", "for", "in", "if", "else", "table", "each"],
    "hx": ["package", "import", "class", "function", "var", "trace", "Void", "String"],
    "llvm": ["define", "target", "datalayout", "triple", "alloca", "load", "store", "br", "ret", "i32", "i64"],
}

def build():
    print(f"Fetching frequencies from {URL}...")
    req = urllib.request.Request(URL, headers={"User-Agent": "Mozilla/5.0"})
    with urllib.request.urlopen(req) as resp:
        data = resp.read().decode("utf-8")

    blocks = re.findall(r'\"([^\"]+)\": map\[string\]float64\{([^}]+)\}', data)
    raw_token_probs = {}
    for lang, block in blocks:
        raw_token_probs[lang] = {}
        for line in block.splitlines():
            m = re.match(r'\"([^\"]+)\":\s+(-?[\d\.]+)', line.strip())
            if m:
                raw_token_probs[lang][m.group(1)] = round(float(m.group(2)), 3)

    compiled = {}
    for linguist_name, code in LANG_MAP.items():
        if linguist_name not in raw_token_probs:
            continue
        tokens = dict(raw_token_probs[linguist_name])
        if code in compiled:
            compiled[code]["tokens"].update(tokens)
        else:
            compiled[code] = {
                "rank": POPULARITY_RANK.get(code, 20),
                "tokens": tokens
            }

    # Inject distinctive keywords
    for code, kws in LANGUAGE_KEYWORDS.items():
        if code not in compiled:
            compiled[code] = {"rank": POPULARITY_RANK.get(code, 20), "tokens": {}}
        for kw in kws:
            compiled[code]["tokens"][kw] = -0.1

    out_dir = os.path.join(os.path.dirname(__file__), "..", "crates", "little-clip-core", "src", "syntax")
    os.makedirs(out_dir, exist_ok=True)
    out_path = os.path.join(out_dir, "classifier_data.bin.gz")

    json_data = json.dumps(compiled, separators=(",", ":")).encode("utf-8")
    compressed = gzip.compress(json_data, 9)

    with open(out_path, "wb") as f:
        f.write(compressed)

    print(f"Successfully generated {out_path} ({len(compressed) / 1024:.2f} KB, {len(compiled)} languages)")

if __name__ == "__main__":
    build()
