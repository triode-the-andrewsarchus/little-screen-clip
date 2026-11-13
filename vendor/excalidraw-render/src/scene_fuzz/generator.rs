//! The fixed-seed input generator: a lexical JSON model spelled compactly or
//! with arbitrary whitespace and escapes, a structured scene builder over every
//! element kind and field disposition, and the byte-level, envelope, parser
//! ceiling, and token-soup strategies around it.

use std::collections::BTreeSet;
use std::fmt::Write as _;

use crate::ErrorCode;

use super::{
    Expect, GeneratedCase, PointerExpectation, MAX_ELEMENTS, MAX_MUTATIONS, MAX_NESTING,
    MAX_POINTS, STRATEGIES,
};

/// A JSON value with its number tokens kept lexically, so a generated scene
/// can be spelled both compactly (the canonical form) and with arbitrary
/// whitespace and escape spelling.
#[derive(Clone, Debug)]
enum Node {
    Null,
    Bool(bool),
    Number(String),
    Str(String),
    Array(Vec<Node>),
    Object(Vec<(String, Node)>),
}

impl Node {
    fn string(value: impl Into<String>) -> Self {
        Self::Str(value.into())
    }

    fn number(token: impl Into<String>) -> Self {
        Self::Number(token.into())
    }

    fn object(members: Vec<(&str, Node)>) -> Self {
        Self::Object(
            members
                .into_iter()
                .map(|(key, value)| (key.to_owned(), value))
                .collect(),
        )
    }

    fn member_mut(&mut self, key: &str) -> Option<&mut Node> {
        let Self::Object(members) = self else {
            return None;
        };
        members
            .iter_mut()
            .find(|(member, _)| member == key)
            .map(|(_, value)| value)
    }

    fn remove_member(&mut self, key: &str) {
        if let Self::Object(members) = self {
            members.retain(|(member, _)| member != key);
        }
    }

    /// The canonical spelling of spec §4.2: minified, short escapes for the
    /// named controls, lowercase `\u00xx` for other controls, everything else
    /// direct, number tokens verbatim.
    fn emit_compact(&self, out: &mut String) {
        match self {
            Self::Null => out.push_str("null"),
            Self::Bool(value) => out.push_str(if *value { "true" } else { "false" }),
            Self::Number(token) => out.push_str(token),
            Self::Str(value) => push_canonical_string(out, value),
            Self::Array(values) => {
                out.push('[');
                for (index, value) in values.iter().enumerate() {
                    if index != 0 {
                        out.push(',');
                    }
                    value.emit_compact(out);
                }
                out.push(']');
            }
            Self::Object(members) => {
                out.push('{');
                for (index, (key, value)) in members.iter().enumerate() {
                    if index != 0 {
                        out.push(',');
                    }
                    push_canonical_string(out, key);
                    out.push(':');
                    value.emit_compact(out);
                }
                out.push('}');
            }
        }
    }

    /// The same document with random insignificant whitespace and random
    /// but valid escape spellings.
    fn emit_respelled(&self, generator: &mut Generator, out: &mut String) {
        push_whitespace(generator, out);
        match self {
            Self::Null => out.push_str("null"),
            Self::Bool(value) => out.push_str(if *value { "true" } else { "false" }),
            Self::Number(token) => out.push_str(token),
            Self::Str(value) => push_respelled_string(generator, out, value),
            Self::Array(values) => {
                out.push('[');
                for (index, value) in values.iter().enumerate() {
                    if index != 0 {
                        push_whitespace(generator, out);
                        out.push(',');
                    }
                    value.emit_respelled(generator, out);
                }
                push_whitespace(generator, out);
                out.push(']');
            }
            Self::Object(members) => {
                out.push('{');
                for (index, (key, value)) in members.iter().enumerate() {
                    if index != 0 {
                        push_whitespace(generator, out);
                        out.push(',');
                    }
                    push_whitespace(generator, out);
                    push_respelled_string(generator, out, key);
                    push_whitespace(generator, out);
                    out.push(':');
                    value.emit_respelled(generator, out);
                }
                push_whitespace(generator, out);
                out.push('}');
            }
        }
        push_whitespace(generator, out);
    }
}

fn push_canonical_string(out: &mut String, value: &str) {
    out.push('"');
    for character in value.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{0008}' => out.push_str("\\b"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\u{000c}' => out.push_str("\\f"),
            '\r' => out.push_str("\\r"),
            '\u{0000}'..='\u{001f}' => {
                let _ = write!(out, "\\u{:04x}", u32::from(character));
            }
            character => out.push(character),
        }
    }
    out.push('"');
}

fn push_respelled_string(generator: &mut Generator, out: &mut String, value: &str) {
    out.push('"');
    for character in value.chars() {
        let escape_anyway = generator.below(6) == 0;
        match character {
            '"' if generator.below(2) == 0 => out.push_str("\\\""),
            '\\' if generator.below(2) == 0 => out.push_str("\\\\"),
            '/' if generator.below(2) == 0 => out.push_str("\\/"),
            '\u{0008}' if !escape_anyway => out.push_str("\\b"),
            '\t' if !escape_anyway => out.push_str("\\t"),
            '\n' if !escape_anyway => out.push_str("\\n"),
            '\u{000c}' if !escape_anyway => out.push_str("\\f"),
            '\r' if !escape_anyway => out.push_str("\\r"),
            '"' | '\\' | '\u{0000}'..='\u{001f}' => push_unicode_escape(generator, out, character),
            character if escape_anyway => push_unicode_escape(generator, out, character),
            character => out.push(character),
        }
    }
    out.push('"');
}

fn push_unicode_escape(generator: &mut Generator, out: &mut String, character: char) {
    let mut units = [0_u16; 2];
    for unit in character.encode_utf16(&mut units) {
        if generator.below(2) == 0 {
            let _ = write!(out, "\\u{unit:04x}");
        } else {
            let _ = write!(out, "\\u{unit:04X}");
        }
    }
}

fn push_whitespace(generator: &mut Generator, out: &mut String) {
    for _ in 0..generator.below(3) {
        out.push(match generator.below(4) {
            0 => ' ',
            1 => '\t',
            2 => '\n',
            _ => '\r',
        });
    }
}

const ONE_PIXEL_PNG_DATA_URL: &str = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==";

const COORDINATE_TOKENS: [&str; 12] = [
    "0",
    "-0",
    "10",
    "20.5",
    "-15",
    "100",
    "1e2",
    "2.5E+1",
    "1234.5678",
    "-0.0",
    "3.0e0",
    "0.000001",
];
const DIMENSION_TOKENS: [&str; 9] = ["40", "0", "-40", "120.5", "1e2", "-0", "0.5", "1E1", "300"];
/// Magnitudes past every coordinate ceiling; rare, so most scenes still reach
/// the encoder instead of failing at the dimension check.
const EXTREME_TOKENS: [&str; 6] = [
    "1e300",
    "-1e300",
    "1e21",
    "123456789012345678901234567890",
    "-1e-300",
    "4.9e-324",
];
const COLOR_TOKENS: [&str; 12] = [
    "#1e1e1e",
    "#ffffff",
    "#e03131",
    "transparent",
    "red",
    "rgb(1, 2, 3)",
    "not-a-color",
    "",
    "#12",
    "hsl(0, 0%, 0%)",
    "#ffffff80",
    "  #000000  ",
];
const LINK_TOKENS: [&str; 14] = [
    "https://example.com/a?b=c#d",
    "http://x.y",
    "/relative/path",
    "./rel",
    "javascript:alert(1)",
    "  data:text/html,hi",
    "&#106;avascript:x",
    "&#38;#106;avascript:x",
    "mailto:a@b.c",
    "",
    "   ",
    "vbscript:x",
    "&Tab;https://e.com",
    "https://e.com/\u{2028}\u{feff}z",
];
const TEXT_TOKENS: [&str; 9] = [
    "A",
    "Roadmap",
    "\u{4e2d}\u{6587}",
    "\u{1f600}",
    "One two\nthree",
    "\r\nline\r",
    "\u{5d0}\u{5d1}\u{5d2} abc",
    "",
    "tab\there \"quoted\" back\\slash /slash",
];
const IDENTITY_TOKENS: [&str; 8] = [
    "a",
    "b",
    "same",
    "",
    "x/y",
    "~tilde",
    "\u{e9}\u{2028}",
    "same",
];
const FRACTIONAL_INDEX_TOKENS: [&str; 9] = ["a0", "a1", "a2", "a1V", "Zz", "a0G", "foo", "", "a00"];

impl Generator {
    fn pick<'a>(&mut self, tokens: &[&'a str]) -> &'a str {
        tokens[self.below(tokens.len() as u64) as usize]
    }

    fn chance(&mut self, denominator: u64) -> bool {
        self.below(denominator) == 0
    }

    /// A value that is usually a valid number token and sometimes a wrong
    /// type, so every numeric field sees its invalid disposition.
    fn numeric(&mut self, tokens: &[&str]) -> Node {
        if self.chance(12) {
            return self.wrong_type();
        }
        if self.chance(40) {
            return Node::number(self.pick(&EXTREME_TOKENS));
        }
        Node::number(self.pick(tokens))
    }

    fn wrong_type(&mut self) -> Node {
        match self.below(5) {
            0 => Node::Null,
            1 => Node::Bool(true),
            2 => Node::string("wrong"),
            3 => Node::Array(Vec::new()),
            _ => Node::object(vec![("nested", Node::number("1"))]),
        }
    }

    fn text_value(&mut self, tokens: &[&str]) -> Node {
        if self.chance(12) {
            return self.wrong_type();
        }
        Node::string(self.pick(tokens))
    }

    fn point(&mut self) -> Node {
        match self.below(24) {
            0 => Node::Array(vec![
                self.numeric(&COORDINATE_TOKENS),
                self.numeric(&COORDINATE_TOKENS),
                Node::number("7"),
            ]),
            1 => Node::Array(vec![self.numeric(&COORDINATE_TOKENS)]),
            2 => self.wrong_type(),
            3 => Node::Array(vec![Node::string("x"), Node::number("1")]),
            _ => Node::Array(vec![
                Node::number(self.pick(&COORDINATE_TOKENS)),
                Node::number(self.pick(&COORDINATE_TOKENS)),
            ]),
        }
    }

    fn points(&mut self) -> Node {
        if self.chance(16) {
            return self.wrong_type();
        }
        let count = match self.below(6) {
            0 => 0,
            1 => 1,
            2 => 2,
            _ => 2 + self.below(MAX_POINTS as u64 - 2) as usize,
        };
        Node::Array((0..count).map(|_| self.point()).collect())
    }

    fn binding(&mut self) -> Node {
        if self.chance(6) {
            return Node::Null;
        }
        if self.chance(16) {
            return self.wrong_type();
        }
        let mut members = Vec::new();
        match self.below(6) {
            0 => {}
            1 => members.push(("elementId", Node::string(""))),
            2 => members.push(("elementId", Node::number("4"))),
            _ => members.push(("elementId", Node::string(self.pick(&["a", "b", "target"])))),
        }
        if !self.chance(5) {
            members.push(("focus", self.numeric(&["0", "0.5", "-1", "1", "0.25"])));
        } else if self.chance(2) {
            members.push(("focus", Node::Bool(false)));
        }
        if !self.chance(5) {
            members.push(("gap", self.numeric(&["1", "0", "8", "-2", "1.5"])));
        }
        if self.chance(3) {
            members.push((
                "fixedPoint",
                match self.below(5) {
                    0 => Node::Array(vec![Node::number("0.5"), Node::number("0.5")]),
                    1 => Node::Array(vec![Node::number("0.50005"), Node::number("0.1")]),
                    2 => Node::Null,
                    3 => Node::Array(vec![Node::number("0.2")]),
                    _ => Node::Array(vec![Node::number("0.25"), Node::number("0.75")]),
                },
            ));
        }
        if self.chance(6) {
            members.push(("mode", Node::string("orbit")));
        }
        Node::object(members)
    }

    fn crop(&mut self) -> Node {
        if self.chance(6) {
            return Node::Null;
        }
        if self.chance(12) {
            return self.wrong_type();
        }
        let mut members = Vec::new();
        for (field, tokens) in [
            ("x", &["0", "1", "-1", "2.5"][..]),
            ("y", &["0", "2", "-3"][..]),
            ("width", &["30", "0", "-30", "10.5"][..]),
            ("height", &["20", "0", "20.25"][..]),
            ("naturalWidth", &["60", "0", "64"][..]),
            ("naturalHeight", &["40", "48", "-1"][..]),
        ] {
            if !self.chance(10) {
                members.push((field, self.numeric(tokens)));
            }
        }
        if self.chance(5) {
            members.push(("rotation", Node::number("90")));
        }
        Node::object(members)
    }

    fn roundness(&mut self) -> Node {
        match self.below(8) {
            0 => Node::Null,
            1 => self.wrong_type(),
            2 => Node::object(vec![("type", Node::number("4"))]),
            3 => Node::object(vec![
                ("type", Node::number("3")),
                ("value", self.numeric(&["8", "-1", "32", "0"])),
            ]),
            4 => Node::object(vec![("type", Node::number("1"))]),
            5 => Node::object(vec![("type", Node::string("2"))]),
            _ => Node::object(vec![("type", Node::number("2"))]),
        }
    }

    fn element(&mut self, index: usize, frame_count: usize, file_keys: &[String]) -> Node {
        if self.chance(24) {
            return match self.below(3) {
                0 => Node::Null,
                1 => Node::number("42"),
                _ => Node::Array(vec![Node::string("not"), Node::string("an element")]),
            };
        }
        let kind = self.pick(&[
            "rectangle",
            "rectangle",
            "diamond",
            "ellipse",
            "text",
            "text",
            "line",
            "arrow",
            "arrow",
            "draw",
            "freedraw",
            "image",
            "frame",
            "magicframe",
            "iframe",
            "embeddable",
            "custom-widget",
            "selection",
        ]);
        let mut members: Vec<(&str, Node)> = Vec::new();

        match self.below(8) {
            0 => {}
            1 => members.push(("id", Node::number("9"))),
            2 | 3 => members.push(("id", Node::string(self.pick(&IDENTITY_TOKENS)))),
            _ => members.push(("id", Node::string(format!("{kind}-{index}")))),
        }
        members.push(("type", Node::string(kind)));
        match self.below(6) {
            0 => {}
            1 => members.push(("index", Node::Null)),
            2 => members.push(("index", self.wrong_type())),
            _ => members.push(("index", Node::string(self.pick(&FRACTIONAL_INDEX_TOKENS)))),
        }
        for field in ["x", "y"] {
            if !self.chance(8) {
                members.push((field, self.numeric(&COORDINATE_TOKENS)));
            }
        }
        for field in ["width", "height"] {
            if !self.chance(8) {
                members.push((field, self.numeric(&DIMENSION_TOKENS)));
            }
        }
        if self.chance(2) {
            members.push((
                "angle",
                self.numeric(&["0", "1.5707963267948966", "3.14159", "-0.5", "6.5", "1e10"]),
            ));
        }
        if self.chance(2) {
            members.push(("strokeColor", self.text_value(&COLOR_TOKENS)));
        }
        if self.chance(2) {
            members.push(("backgroundColor", self.text_value(&COLOR_TOKENS)));
        }
        if self.chance(2) {
            members.push((
                "fillStyle",
                self.text_value(&["hachure", "cross-hatch", "solid", "zigzag", "dots"]),
            ));
        }
        if self.chance(2) {
            members.push((
                "strokeWidth",
                self.numeric(&["1", "2", "4", "0", "-1", "0.5"]),
            ));
        }
        if self.chance(2) {
            members.push((
                "strokeStyle",
                self.text_value(&["solid", "dashed", "dotted", "wavy"]),
            ));
        }
        if self.chance(2) {
            members.push(("roughness", self.numeric(&["0", "1", "2", "-1", "1.5"])));
        }
        if self.chance(2) {
            members.push((
                "opacity",
                self.numeric(&["100", "50", "0", "150", "-1", "100.0", "1e2"]),
            ));
        }
        if !self.chance(4) {
            members.push((
                "seed",
                self.numeric(&["0", "1", "12345", "-7", "2147483647", "2147483648", "1.5"]),
            ));
        }
        if self.chance(3) {
            members.push(("roundness", self.roundness()));
        }
        match self.below(8) {
            0 => members.push(("isDeleted", Node::Bool(true))),
            1 => members.push(("isDeleted", Node::string("true"))),
            2 | 3 => members.push(("isDeleted", Node::Bool(false))),
            _ => {}
        }
        match self.below(6) {
            0 => members.push(("groupIds", Node::Array(vec![Node::string("g1")]))),
            1 => members.push(("groupIds", Node::string("nope"))),
            2 => members.push(("groupIds", Node::Array(Vec::new()))),
            _ => {}
        }
        match self.below(6) {
            0 if frame_count > 0 => members.push((
                "frameId",
                Node::string(format!("frame-{}", self.below(frame_count as u64))),
            )),
            0 | 1 => members.push(("frameId", Node::string("missing-frame"))),
            2 => members.push(("frameId", Node::Null)),
            3 => members.push(("frameId", Node::string(""))),
            _ => {}
        }
        match self.below(6) {
            0 => members.push((
                "boundElements",
                Node::Array(vec![Node::object(vec![
                    ("id", Node::string("text-1")),
                    ("type", Node::string("text")),
                ])]),
            )),
            1 => members.push(("boundElements", Node::Bool(true))),
            2 => members.push(("boundElements", Node::Null)),
            _ => {}
        }
        match self.below(4) {
            0 => members.push(("link", Node::string(self.pick(&LINK_TOKENS)))),
            1 => members.push(("link", Node::Null)),
            _ => {}
        }
        if !self.chance(4) {
            members.push(("updated", self.numeric(&["1", "0", "1700000000000", "-1"])));
        }
        if self.chance(3) {
            members.push(("locked", Node::Bool(self.chance(2))));
        }
        if self.chance(4) {
            members.push((
                "futureField",
                Node::object(vec![("nested", Node::Bool(true))]),
            ));
        }
        if self.chance(6) {
            members.push(("customData", Node::object(vec![("k", Node::string("v"))])));
        }
        if self.chance(10) {
            members.push(("~escaped/key", Node::number("1")));
        }

        match kind {
            "text" => {
                if !self.chance(8) {
                    members.push(("text", self.text_value(&TEXT_TOKENS)));
                }
                if self.chance(2) {
                    members.push(("originalText", self.text_value(&TEXT_TOKENS)));
                }
                if !self.chance(4) {
                    members.push(("fontSize", self.numeric(&["20", "16", "28.5", "0", "-3"])));
                }
                if !self.chance(4) {
                    members.push((
                        "fontFamily",
                        self.numeric(&[
                            "1", "2", "3", "4", "5", "6", "7", "8", "9", "10", "0", "1.5",
                        ]),
                    ));
                }
                if self.chance(2) {
                    members.push((
                        "textAlign",
                        self.text_value(&["left", "center", "right", "justify"]),
                    ));
                }
                if self.chance(2) {
                    members.push((
                        "verticalAlign",
                        self.text_value(&["top", "middle", "bottom", "baseline"]),
                    ));
                }
                if self.chance(2) {
                    members.push((
                        "lineHeight",
                        self.numeric(&["1.25", "1.15", "1.5", "0", "-1", "1"]),
                    ));
                }
                if self.chance(4) {
                    members.push(("containerId", Node::string("rectangle-0")));
                }
                if self.chance(4) {
                    members.push(("autoResize", Node::Bool(false)));
                }
            }
            "line" | "arrow" | "draw" => {
                if !self.chance(6) {
                    members.push(("points", self.points()));
                }
                if self.chance(2) {
                    members.push(("startBinding", self.binding()));
                }
                if self.chance(2) {
                    members.push(("endBinding", self.binding()));
                }
                for field in ["startArrowhead", "endArrowhead"] {
                    match self.below(5) {
                        0 => members.push((field, Node::Null)),
                        1 => members.push((
                            field,
                            self.text_value(&[
                                "arrow",
                                "bar",
                                "dot",
                                "triangle",
                                "crowfoot_one",
                                "spear",
                            ]),
                        )),
                        _ => {}
                    }
                }
                match self.below(5) {
                    0 => members.push(("elbowed", Node::Bool(true))),
                    1 => members.push(("elbowed", Node::string("yes"))),
                    _ => {}
                }
                if self.chance(4) {
                    members.push((
                        "lastCommittedPoint",
                        Node::Array(vec![Node::number("1"), Node::number("2")]),
                    ));
                }
            }
            "freedraw" => {
                if !self.chance(6) {
                    members.push(("points", self.points()));
                }
                match self.below(5) {
                    0 => {}
                    1 => members.push(("pressures", self.wrong_type())),
                    2 => members.push((
                        "pressures",
                        Node::Array(vec![Node::number("0.5"), Node::number("1.5")]),
                    )),
                    _ => {
                        let count = self.below(MAX_POINTS as u64) as usize;
                        members.push((
                            "pressures",
                            Node::Array(
                                (0..count)
                                    .map(|_| {
                                        Node::number(self.pick(&["0", "0.25", "0.5", "1", "0.75"]))
                                    })
                                    .collect(),
                            ),
                        ));
                    }
                }
                match self.below(4) {
                    0 => members.push(("simulatePressure", Node::Bool(false))),
                    1 => members.push(("simulatePressure", Node::number("1"))),
                    _ => members.push(("simulatePressure", Node::Bool(true))),
                }
            }
            "image" => {
                match self.below(6) {
                    0 => members.push(("fileId", Node::Null)),
                    1 => members.push(("fileId", Node::number("3"))),
                    2 => members.push(("fileId", Node::string("absent-file"))),
                    _ if !file_keys.is_empty() => {
                        let key = &file_keys[self.below(file_keys.len() as u64) as usize];
                        members.push(("fileId", Node::string(key.clone())));
                    }
                    _ => members.push(("fileId", Node::string("absent-file"))),
                }
                if self.chance(2) {
                    members.push((
                        "status",
                        self.text_value(&["pending", "saved", "error", "loading"]),
                    ));
                }
                match self.below(6) {
                    0 => members.push((
                        "scale",
                        Node::Array(vec![Node::number("-1"), Node::number("1")]),
                    )),
                    1 => members.push((
                        "scale",
                        Node::Array(vec![Node::number("2"), Node::number("1")]),
                    )),
                    2 => members.push(("scale", Node::Array(vec![Node::number("1")]))),
                    3 => members.push(("scale", self.wrong_type())),
                    _ => members.push((
                        "scale",
                        Node::Array(vec![Node::number("1"), Node::number("1")]),
                    )),
                }
                if self.chance(2) {
                    members.push(("crop", self.crop()));
                }
            }
            "frame" | "magicframe" => match self.below(4) {
                0 => members.push(("name", Node::Null)),
                1 => members.push(("name", self.wrong_type())),
                2 => members.push(("name", Node::string(format!("Frame {index}")))),
                _ => {}
            },
            _ => {}
        }

        if self.chance(6) {
            let swap = self.below(members.len() as u64) as usize;
            members.swap(0, swap);
        }
        Node::object(members)
    }

    fn file_record(&mut self, key: &str) -> Node {
        if self.chance(8) {
            return match self.below(4) {
                0 => Node::Bool(false),
                1 => Node::Null,
                2 => Node::number("42"),
                _ => Node::string("record"),
            };
        }
        let mut members = Vec::new();
        match self.below(6) {
            0 => {}
            1 => members.push(("id", Node::string(""))),
            2 => members.push(("id", Node::number("1"))),
            3 => members.push(("id", Node::string("other-id"))),
            _ => members.push(("id", Node::string(key))),
        }
        match self.below(6) {
            0 => {}
            1 => members.push(("mimeType", Node::string("text/html"))),
            2 => members.push(("mimeType", Node::Bool(true))),
            _ => members.push(("mimeType", Node::string("image/png"))),
        }
        match self.below(6) {
            0 => {}
            1 => members.push(("dataURL", Node::string("data:image/png;base64,AAAA"))),
            2 => members.push(("dataURL", Node::string("https://example.com/a.png"))),
            3 => members.push(("dataURL", Node::number("0"))),
            _ => members.push(("dataURL", Node::string(ONE_PIXEL_PNG_DATA_URL))),
        }
        match self.below(6) {
            0 => {}
            1 => members.push(("created", Node::number("-1"))),
            2 => members.push(("created", Node::number("1.5"))),
            _ => members.push((
                "created",
                Node::number(self.pick(&["1", "0", "1700000000000"])),
            )),
        }
        if self.chance(4) {
            members.push(("lastRetrieved", Node::number("5")));
        }
        Node::object(members)
    }

    fn app_state(&mut self) -> Node {
        let mut members = Vec::new();
        match self.below(5) {
            0 => {}
            1 => members.push(("viewBackgroundColor", self.wrong_type())),
            _ => members.push((
                "viewBackgroundColor",
                Node::string(self.pick(&COLOR_TOKENS)),
            )),
        }
        match self.below(4) {
            0 => {}
            1 => members.push(("frameRendering", self.wrong_type())),
            _ => {
                let mut controls = Vec::new();
                for field in ["enabled", "clip", "name", "outline"] {
                    match self.below(5) {
                        0 => {}
                        1 => controls.push((field, Node::string("yes"))),
                        _ => controls.push((field, Node::Bool(self.chance(2)))),
                    }
                }
                if self.chance(4) {
                    controls.push(("labels", Node::Bool(true)));
                }
                members.push(("frameRendering", Node::object(controls)));
            }
        }
        if self.chance(3) {
            members.push(("gridSize", Node::Null));
        }
        if self.chance(3) {
            members.push(("zoom", Node::object(vec![("value", Node::number("1E+00"))])));
        }
        if self.chance(4) {
            members.push((
                "futureState",
                Node::Array(vec![Node::number("3.00"), Node::number("1e-2")]),
            ));
        }
        Node::object(members)
    }

    /// One admissible scene document over every element kind, with fields
    /// present, missing, or invalid in every disposition the manifest names.
    fn scene_model(&mut self) -> Node {
        let file_count = self.below(4) as usize;
        let file_keys = (0..file_count)
            .map(|index| match self.below(6) {
                0 => String::new(),
                1 => "same".to_owned(),
                _ => format!("file-{index}"),
            })
            .collect::<Vec<_>>();
        let element_count = self.below(MAX_ELEMENTS as u64 + 1) as usize;
        let mut elements = Vec::new();
        let mut frame_count = 0;
        for index in 0..element_count {
            let element = self.element(index, frame_count, &file_keys);
            // Frames are identified as `frame-N`/`magicframe-N` by element
            // index, so a `frame-K` reference for `K < frame_count` sometimes
            // resolves and sometimes dangles; both are wanted.
            if let Node::Object(members) = &element {
                if matches!(
                    members.iter().find(|(key, _)| key == "type"),
                    Some((_, Node::Str(kind))) if kind == "frame" || kind == "magicframe"
                ) {
                    frame_count += 1;
                }
            }
            elements.push(element);
        }

        let mut files = Vec::new();
        let mut seen_keys = BTreeSet::new();
        for key in &file_keys {
            if !seen_keys.insert(key.clone()) {
                continue;
            }
            files.push((key.clone(), self.file_record(key)));
        }

        let mut root = vec![
            ("type".to_owned(), Node::string("excalidraw")),
            ("version".to_owned(), Node::number("2")),
            ("elements".to_owned(), Node::Array(elements)),
            ("appState".to_owned(), self.app_state()),
            ("files".to_owned(), Node::Object(files)),
        ];
        if self.chance(3) {
            root.push(("source".to_owned(), Node::string("https://excalidraw.com")));
        }
        if self.chance(6) {
            root.push((
                "futureRoot".to_owned(),
                Node::Array(vec![Node::number("-0")]),
            ));
        }
        for _ in 0..self.below(3) {
            let from = self.below(root.len() as u64) as usize;
            let to = self.below(root.len() as u64) as usize;
            root.swap(from, to);
        }
        Node::Object(root)
    }
}

/// Deterministic xorshift generator; the corpus must be reproducible on every
/// target, so no environment or clock input is admitted.
pub(super) struct Generator(pub(super) u64);

impl Generator {
    fn next_u64(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, bound: u64) -> u64 {
        self.next_u64() % bound.max(1)
    }

    fn next_u8(&mut self) -> u8 {
        self.next_u64().to_le_bytes()[0]
    }

    /// One case per strategy in rotation, so every sample size covers every
    /// strategy evenly.
    pub(super) fn case(&mut self, index: usize) -> GeneratedCase {
        match index % STRATEGIES {
            0..=2 => {
                let model = self.scene_model();
                let mut input = String::new();
                model.emit_compact(&mut input);
                GeneratedCase {
                    input: input.into_bytes(),
                    expect: Expect::Admitted,
                }
            }
            3 => {
                let model = self.scene_model();
                let mut compact = String::new();
                model.emit_compact(&mut compact);
                let mut respelled = String::new();
                model.emit_respelled(self, &mut respelled);
                GeneratedCase {
                    input: respelled.into_bytes(),
                    expect: Expect::Canonical(compact.into_bytes()),
                }
            }
            4 => self.broken_envelope(),
            5 => self.mutated_scene(),
            6 => self.adversarial(),
            _ => self.token_soup(),
        }
    }

    /// One envelope member broken in a way the envelope check must name.
    fn broken_envelope(&mut self) -> GeneratedCase {
        let mut model = self.scene_model();
        let (pointer, break_member): (&str, fn(&mut Generator, &mut Node)) = match self.below(6) {
            0 => ("", |_, root| {
                *root = Node::Array(vec![std::mem::replace(root, Node::Null)]);
            }),
            1 => ("/type", |generator, root| match generator.below(4) {
                0 => root.remove_member("type"),
                1 => *root.member_mut("type").expect("type") = Node::string("Excalidraw"),
                2 => *root.member_mut("type").expect("type") = Node::number("2"),
                _ => *root.member_mut("type").expect("type") = Node::Null,
            }),
            2 => ("/version", |generator, root| match generator.below(5) {
                0 => root.remove_member("version"),
                1 => *root.member_mut("version").expect("version") = Node::string("2"),
                2 => *root.member_mut("version").expect("version") = Node::number("2.0"),
                3 => *root.member_mut("version").expect("version") = Node::number("1"),
                _ => *root.member_mut("version").expect("version") = Node::number("2e0"),
            }),
            3 => ("/elements", |generator, root| match generator.below(3) {
                0 => root.remove_member("elements"),
                1 => *root.member_mut("elements").expect("elements") = Node::Object(Vec::new()),
                _ => *root.member_mut("elements").expect("elements") = Node::Null,
            }),
            // An absent appState or files member is admitted, as Excalidraw
            // restores it; only another kind breaks the envelope.
            4 => ("/appState", |generator, root| match generator.below(2) {
                0 => *root.member_mut("appState").expect("appState") = Node::Array(Vec::new()),
                _ => *root.member_mut("appState").expect("appState") = Node::string("{}"),
            }),
            _ => ("/files", |generator, root| match generator.below(2) {
                0 => *root.member_mut("files").expect("files") = Node::Array(Vec::new()),
                _ => *root.member_mut("files").expect("files") = Node::Bool(false),
            }),
        };
        break_member(self, &mut model);
        let mut input = String::new();
        model.emit_compact(&mut input);
        GeneratedCase {
            input: input.into_bytes(),
            expect: Expect::Rejected {
                code: ErrorCode::SCENE_INVALID_ENVELOPE,
                json_pointer: PointerExpectation::Exactly(pointer.to_owned()),
            },
        }
    }

    /// A structured scene with bounded byte-level damage.
    fn mutated_scene(&mut self) -> GeneratedCase {
        let model = self.scene_model();
        let mut input = String::new();
        model.emit_compact(&mut input);
        let mut bytes = input.into_bytes();
        let alphabet: &[u8] = b"{}[],:\"\\0123456789-.eE+truefalsenull \t\n\r/u~";
        for _ in 0..1 + self.below(MAX_MUTATIONS as u64) {
            if bytes.is_empty() {
                break;
            }
            let offset = self.below(bytes.len() as u64) as usize;
            match self.below(7) {
                0 => bytes[offset] ^= self.next_u8() | 1,
                1 => {
                    bytes.remove(offset);
                }
                2 => {
                    let byte = alphabet[self.below(alphabet.len() as u64) as usize];
                    bytes.insert(offset, byte);
                }
                3 => bytes.insert(offset, self.next_u8()),
                4 => bytes.truncate(offset),
                5 => {
                    let end = (offset + 1 + self.below(32) as usize).min(bytes.len());
                    let slice = bytes[offset..end].to_vec();
                    let target = self.below(bytes.len() as u64 + 1) as usize;
                    bytes.splice(target..target, slice);
                }
                _ => {
                    let byte = alphabet[self.below(alphabet.len() as u64) as usize];
                    bytes[offset] = byte;
                }
            }
        }
        GeneratedCase {
            input: bytes,
            expect: Expect::Any,
        }
    }

    /// Inputs aimed at the fixed parser envelope and lexical corner cases.
    fn adversarial(&mut self) -> GeneratedCase {
        match self.below(12) {
            0 => {
                let nested = match self.below(4) {
                    0 => 125 + self.below(6) as usize,
                    1 => self.below(200) as usize,
                    2 => 1_000 + self.below(4_000) as usize,
                    _ => MAX_NESTING,
                };
                let input = nested_arrays_scene(nested);
                let expect = if 2 + nested <= 128 {
                    Expect::Admitted
                } else {
                    Expect::Rejected {
                        code: ErrorCode::SCENE_JSON_DEPTH_EXCEEDED,
                        json_pointer: PointerExpectation::Unchecked,
                    }
                };
                GeneratedCase { input, expect }
            }
            1 => {
                let nested = 120 + self.below(12) as usize;
                let mut input = String::from(
                    r#"{"type":"excalidraw","version":2,"elements":[],"appState":{"a":"#,
                );
                for _ in 0..nested {
                    input.push_str(r#"{"a":"#);
                }
                input.push_str("null");
                for _ in 0..nested {
                    input.push('}');
                }
                input.push_str(r#"},"files":{}}"#);
                // The root is depth 1, `appState` depth 2, and each nested
                // object one more.
                let expect = if 2 + nested <= 128 {
                    Expect::Admitted
                } else {
                    Expect::Rejected {
                        code: ErrorCode::SCENE_JSON_DEPTH_EXCEEDED,
                        json_pointer: PointerExpectation::Unchecked,
                    }
                };
                GeneratedCase {
                    input: input.into_bytes(),
                    expect,
                }
            }
            2 => {
                let length = match self.below(3) {
                    0 => 4_095 + self.below(3) as usize,
                    1 => self.below(4_096) as usize,
                    _ => 4_097 + self.below(4_096) as usize,
                };
                let key = "k".repeat(length);
                let input = format!(
                    r#"{{"type":"excalidraw","version":2,"elements":[],"appState":{{"{key}":1}},"files":{{}}}}"#
                );
                let expect = if length <= 4_096 {
                    Expect::Admitted
                } else {
                    // A key that never finished decoding is located by the
                    // object that holds it.
                    Expect::Rejected {
                        code: ErrorCode::SCENE_OBJECT_KEY_TOO_LARGE,
                        json_pointer: PointerExpectation::Exactly("/appState".to_owned()),
                    }
                };
                GeneratedCase {
                    input: input.into_bytes(),
                    expect,
                }
            }
            3 => {
                let length = match self.below(3) {
                    0 => 127 + self.below(3) as usize,
                    1 => 1 + self.below(127) as usize,
                    _ => 129 + self.below(400) as usize,
                };
                let token = "9".repeat(length);
                let input = format!(
                    r#"{{"type":"excalidraw","version":2,"elements":[{token}],"appState":{{}},"files":{{}}}}"#
                );
                let expect = if length <= 128 {
                    Expect::Admitted
                } else {
                    Expect::Rejected {
                        code: ErrorCode::SCENE_NUMBER_TOKEN_TOO_LARGE,
                        json_pointer: PointerExpectation::Exactly("/elements/0".to_owned()),
                    }
                };
                GeneratedCase {
                    input: input.into_bytes(),
                    expect,
                }
            }
            4 => {
                let key = self.pick(&["id", "type", "x", "seed", "~", "/", ""]);
                let input = format!(
                    r#"{{"type":"excalidraw","version":2,"elements":[{{"{key}":1,"{key}":2}}],"appState":{{}},"files":{{}}}}"#
                );
                let mut pointer = String::from("/elements/0/");
                push_pointer_segment(&mut pointer, key);
                GeneratedCase {
                    input: input.into_bytes(),
                    expect: Expect::Rejected {
                        code: ErrorCode::SCENE_DUPLICATE_OBJECT_KEY,
                        json_pointer: PointerExpectation::Exactly(pointer),
                    },
                }
            }
            5 => {
                let model = self.scene_model();
                let mut input = String::new();
                model.emit_compact(&mut input);
                let mut bytes = input.into_bytes();
                let offset = self.below(bytes.len() as u64 + 1) as usize;
                let invalid: &[u8] = match self.below(4) {
                    0 => &[0xff],
                    1 => &[0xc0, 0xaf],
                    2 => &[0xed, 0xa0, 0x80],
                    _ => &[0xf4, 0x90, 0x80, 0x80],
                };
                bytes.splice(offset..offset, invalid.iter().copied());
                GeneratedCase {
                    input: bytes,
                    expect: Expect::Rejected {
                        code: ErrorCode::SCENE_INVALID_UTF8,
                        json_pointer: PointerExpectation::Absent,
                    },
                }
            }
            6 => {
                let model = self.scene_model();
                let mut input = String::from("\u{feff}");
                model.emit_compact(&mut input);
                GeneratedCase {
                    input: input.into_bytes(),
                    expect: Expect::Rejected {
                        code: ErrorCode::SCENE_INVALID_JSON,
                        json_pointer: PointerExpectation::Unchecked,
                    },
                }
            }
            7 => {
                let model = self.scene_model();
                let mut input = String::new();
                model.emit_compact(&mut input);
                input.push_str(self.pick(&["{}", "]", "x", "\u{0}", " null", ","]));
                GeneratedCase {
                    input: input.into_bytes(),
                    expect: Expect::Rejected {
                        code: ErrorCode::SCENE_INVALID_JSON,
                        json_pointer: PointerExpectation::Unchecked,
                    },
                }
            }
            8 => {
                let escape = self.pick(&[
                    r"\ud800",
                    r"\udc00",
                    r"\ud83d\u0041",
                    r"\uZZZZ",
                    r"\x41",
                    r"\u12",
                    "\u{1}",
                ]);
                let input = format!(
                    r#"{{"type":"excalidraw","version":2,"elements":[{{"id":"{escape}"}}],"appState":{{}},"files":{{}}}}"#
                );
                GeneratedCase {
                    input: input.into_bytes(),
                    expect: Expect::Rejected {
                        code: ErrorCode::SCENE_INVALID_JSON,
                        json_pointer: PointerExpectation::Exactly("/elements/0/id".to_owned()),
                    },
                }
            }
            9 => {
                let input = self
                    .pick(&[
                        "",
                        " ",
                        "{",
                        "[]",
                        "{}",
                        "null",
                        "\"excalidraw\"",
                        "{\"type\":\"excalidraw\"",
                        "{\"type\":\"excalidraw\",}",
                        "{\"type\":\"excalidraw\",\"version\":02,\"elements\":[],\"appState\":{},\"files\":{}}",
                        "{\"type\":\"excalidraw\",\"version\":2,\"elements\":[1.],\"appState\":{},\"files\":{}}",
                        "{\"type\":\"excalidraw\",\"version\":2,\"elements\":[-],\"appState\":{},\"files\":{}}",
                        "{\"type\":\"excalidraw\",\"version\":2,\"elements\":[1e],\"appState\":{},\"files\":{}}",
                        "{\"type\":\"excalidraw\",\"version\":2,\"elements\":[tru],\"appState\":{},\"files\":{}}",
                        "{\"type\":\"excalidraw\",\"version\":2,\"elements\":[1 2],\"appState\":{},\"files\":{}}",
                        "{\"type\":\"excalidraw\",\"version\":2,\"elements\":[\"a\nb\"],\"appState\":{},\"files\":{}}",
                    ])
                    .to_owned();
                GeneratedCase {
                    input: input.into_bytes(),
                    expect: Expect::Any,
                }
            }
            10 => {
                // A long but admissible string, spelled directly and escaped.
                let length = 1 + self.below(8_192) as usize;
                let value = if self.chance(2) {
                    "\u{e9}".repeat(length)
                } else {
                    r"\u00e9".repeat(length)
                };
                let input = format!(
                    r#"{{"type":"excalidraw","version":2,"elements":[{{"id":"{value}","type":"rectangle","width":10,"height":10,"seed":1,"updated":1}}],"appState":{{}},"files":{{}}}}"#
                );
                GeneratedCase {
                    input: input.into_bytes(),
                    expect: Expect::Admitted,
                }
            }
            _ => {
                // Keys needing RFC 6901 escaping on an element with repairs,
                // so the repair pointers and the unknown-member pointers both
                // escape.
                let key = self.pick(&["~", "/", "~1", "/0", "a/b~c", "~0~1"]);
                let input = format!(
                    r#"{{"type":"excalidraw","version":2,"elements":[{{"type":"rectangle","width":10,"height":10,"seed":0,"{key}":true}}],"appState":{{"{key}":1}},"files":{{"{key}":{{"id":"{key}"}}}}}}"#
                );
                GeneratedCase {
                    input: input.into_bytes(),
                    expect: Expect::Admitted,
                }
            }
        }
    }

    /// Short random token sequences: mostly invalid JSON, sometimes not.
    fn token_soup(&mut self) -> GeneratedCase {
        const TOKENS: [&str; 24] = [
            "{",
            "}",
            "[",
            "]",
            ",",
            ":",
            "\"",
            "\\",
            "0",
            "1",
            "-",
            ".",
            "e",
            "E",
            "+",
            "true",
            "false",
            "null",
            " ",
            "\n",
            "\"type\"",
            "\"excalidraw\"",
            "\"version\"",
            "2",
        ];
        let length = 1 + self.below(64) as usize;
        let mut input = Vec::new();
        for _ in 0..length {
            if self.chance(16) {
                input.push(self.next_u8());
            } else {
                input
                    .extend_from_slice(TOKENS[self.below(TOKENS.len() as u64) as usize].as_bytes());
            }
        }
        GeneratedCase {
            input,
            expect: Expect::Any,
        }
    }
}

/// A scene whose `elements` array holds `nested` arrays nested inside one
/// another, reaching depth `2 + nested`.
pub(super) fn nested_arrays_scene(nested: usize) -> Vec<u8> {
    let mut input = String::from(r#"{"type":"excalidraw","version":2,"elements":["#);
    input.extend(std::iter::repeat_n('[', nested));
    input.extend(std::iter::repeat_n(']', nested));
    input.push_str(r#"],"appState":{},"files":{}}"#);
    input.into_bytes()
}

/// The pointer of the `nested`-th array inside `elements`, counting the array
/// directly inside `elements` as the first.
pub(super) fn nested_arrays_scene_pointer(nested: usize) -> String {
    let mut pointer = String::from("/elements");
    for _ in 0..nested {
        pointer.push_str("/0");
    }
    pointer
}

/// RFC 6901 segment escaping, spelled independently of the parser's own
/// `push_pointer_segment` so the expected pointers are an oracle rather than
/// an echo.
pub(super) fn push_pointer_segment(pointer: &mut String, segment: &str) {
    for character in segment.chars() {
        match character {
            '~' => pointer.push_str("~0"),
            '/' => pointer.push_str("~1"),
            character => pointer.push(character),
        }
    }
}
