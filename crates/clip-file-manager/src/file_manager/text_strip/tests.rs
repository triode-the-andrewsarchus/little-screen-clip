use super::*;
use std::{
    io::{Cursor, Write},
    sync::atomic::AtomicBool,
};

fn document(name: &str, bytes: &[u8]) -> std::sync::Arc<Document> {
    let folder = tempfile::tempdir().unwrap();
    let path = folder.path().join(name);
    std::fs::write(&path, bytes).unwrap();
    load(&path, &AtomicBool::new(false)).unwrap()
}

#[test]
fn common_syntax_definitions_produce_colored_whole_file_summaries() {
    let examples = [
        ("rs", "// comment\nfn main() { let s = \"hello\"; }\n"),
        ("c", "/* comment */\nint main() { return 42; }\n"),
        (
            "cpp",
            "// comment\nclass Example { public: int value = 42; };\n",
        ),
        ("py", "# comment\ndef example():\n    return \"hello\"\n"),
        (
            "js",
            "// comment\nfunction example() { return \"hello\"; }\n",
        ),
        (
            "ts",
            "// comment\ninterface Example { value: string; }\nconst value = \"hello\";\n",
        ),
        ("sh", "#!/bin/sh\n# comment\necho \"hello\"\n"),
        ("md", "# Heading\n\n**Bold** and `code`\n"),
        ("json", "{\"name\": \"hello\", \"value\": 42}\n"),
        ("yaml", "# comment\nname: \"hello\"\nvalue: true\n"),
        (
            "toml",
            "# comment\n[section]\nname = \"hello\"\nvalue = true\n",
        ),
        ("xml", "<!-- comment -->\n<root name=\"hello\">42</root>\n"),
        ("html", "<!-- comment -->\n<p class=\"hello\">text</p>\n"),
        ("css", "/* comment */\np { color: #abcdef; }\n"),
        (
            "kdl",
            "// comment\nnode name=\"hello\" {\n    child #true\n}\n",
        ),
        ("conf", "# comment\n[section]\nname = \"hello\"\n"),
    ];
    for (extension, source) in examples {
        let doc = document(&format!("example.{extension}"), source.as_bytes());
        assert_eq!(doc.lines, source.lines().count() as u64, "{extension}");
        assert!(
            doc.bins
                .iter()
                .flat_map(|bin| &bin.cells)
                .any(|cell| cell[2..].iter().any(|count| *count > 0)),
            "{extension}: no syntax colors"
        );
        assert_eq!(doc.bins.last().unwrap().end, doc.lines);
    }
}

#[test]
fn named_configuration_files_and_shebangs_select_syntax_without_extensions() {
    let examples = [
        (".env", "# Environment\nNAME=\"example\"\nENABLED=true\n"),
        (".editorconfig", "# Configuration\n[*.rs]\nindent_size=4\n"),
        (".gitconfig", "# Configuration\n[core]\neditor=\"vim\"\n"),
        ("Makefile", "# Make target\nall:\n\t@echo \"example\"\n"),
        (
            "script",
            "#!/usr/bin/env python3\n# Comment\ndef example():\n    return \"hello\"\n",
        ),
    ];
    for (name, source) in examples {
        let doc = document(name, source.as_bytes());
        assert_ne!(doc.language, "Text", "{name}: syntax was not detected");
        assert!(
            doc.bins
                .iter()
                .flat_map(|bin| &bin.cells)
                .any(|cell| cell[1..].iter().any(|count| *count > 0)),
            "{name}: no highlighted syntax"
        );
    }
    let script = document("script", b"#!/usr/bin/env python3\nprint('example')\n");
    assert_eq!(script.badge, "PY");
    assert_eq!(document("notes.unknown", b"Plain notes.\n").badge, "TXT");
}

#[test]
fn typescript_extensions_are_disambiguated_from_transport_streams_off_thread() {
    let folder = tempfile::tempdir().unwrap();
    for extension in ["ts", "mts", "cts"] {
        let source = folder.path().join(format!("code.{extension}"));
        std::fs::write(&source, "interface Example { value: string; }\n").unwrap();
        let video = folder.path().join(format!("video.{extension}"));
        std::fs::write(&video, [0x47, 0x40, 0, 0x10, 0xff, 0xff]).unwrap();
        assert!(is_typescript_source(&source));
        assert!(!is_typescript_source(&video));
    }
    let entries = super::super::filesystem::read_directory(folder.path(), &AtomicBool::new(false))
        .unwrap()
        .entries;
    for entry in entries {
        if entry.name.starts_with("code.") {
            assert!(!entry.is_video());
            assert!(entry.typescript);
        }
        if entry.name == "video.ts" {
            assert!(entry.is_video());
        }
    }
}

#[test]
fn unicode_tabs_and_all_line_endings_have_matching_structure() {
    let sources = [
        "alpha\n\t界x\n\nend\n",
        "alpha\r\n\t界x\r\n\r\nend\r\n",
        "alpha\r\t界x\r\rend\r",
    ];
    let docs: Vec<_> = sources
        .iter()
        .map(|source| document("notes.txt", source.as_bytes()))
        .collect();
    for doc in &docs {
        assert_eq!(doc.lines, 4);
        assert!(
            doc.bins[2]
                .cells
                .iter()
                .all(|cell| cell.iter().all(|count| *count == 0))
        );
        let line = &doc.bins[1];
        assert!(
            line.cells[..4]
                .iter()
                .all(|cell| cell.iter().sum::<u32>() == 0)
        );
        assert!(
            line.cells[4..7]
                .iter()
                .all(|cell| cell.iter().sum::<u32>() == 1)
        );
        assert_eq!(
            line.excerpt[0]
                .runs
                .iter()
                .map(|(text, _)| text.as_str())
                .collect::<String>(),
            "    界x"
        );
    }
    assert_eq!(docs[0].bins[1].cells, docs[1].bins[1].cells);
    assert_eq!(docs[0].bins[1].cells, docs[2].bins[1].cells);
}

#[test]
fn binary_unknown_and_lossy_known_sources_have_explicit_fallbacks() {
    let folder = tempfile::tempdir().unwrap();
    let path = folder.path().join("unknown.file");
    let cancelled = AtomicBool::new(false);
    std::fs::write(&path, b"binary\0payload").unwrap();
    assert!(load(&path, &cancelled).err().unwrap().contains("Binary"));
    std::fs::write(&path, b"invalid \xff data").unwrap();
    assert!(load(&path, &cancelled).err().unwrap().contains("non-UTF-8"));
    let lossy = document("notes.txt", b"invalid \xff data\n");
    assert!(lossy.note.contains("Lossy"));
    let oversized = folder.path().join("oversized.txt");
    std::fs::File::create(&oversized)
        .unwrap()
        .set_len(256 * 1024 * 1024 + 1)
        .unwrap();
    assert!(
        load(&oversized, &cancelled)
            .err()
            .unwrap()
            .contains("256 MiB")
    );
    let plain = document(
        "notes.unknown",
        b"A walk through the garden.\nAnother lovely day.\n",
    );
    assert_eq!(plain.language, "Text");
    assert!(
        plain
            .bins
            .iter()
            .flat_map(|bin| &bin.cells)
            .all(|cell| cell[1..].iter().all(|count| *count == 0))
    );
}

#[test]
fn small_empty_files_long_lines_and_cancellation_are_bounded() {
    assert_eq!(document("empty.txt", b"").lines, 0);
    let long = format!("{}\nlast\n", "界\t".repeat(40_000));
    let doc = document("long.rs", long.as_bytes());
    assert_eq!(doc.lines, 2);
    assert!(doc.note.contains("Monochrome"));
    assert_eq!(doc.bins[1].excerpt[0].number, 2);
    assert!(
        doc.bins[0].excerpt[0]
            .runs
            .iter()
            .map(|(text, _)| text.chars().count())
            .sum::<usize>()
            <= EXCERPT_COLUMNS + 1
    );
    assert!(stream::scan(&mut Cursor::new(b"hello"), &AtomicBool::new(true)).is_err());
    assert!(render(&doc, 500, 66, [[200; 3]; 9], &AtomicBool::new(true)).is_none());
    let combining = format!("a{}end\n", "\u{301}".repeat(10_000));
    let doc = document("combining.txt", combining.as_bytes());
    let excerpt = &doc.bins[0].excerpt[0].runs;
    assert!(
        excerpt
            .iter()
            .map(|(text, _)| text.chars().count())
            .sum::<usize>()
            <= EXCERPT_COLUMNS + 1
    );
    assert!(excerpt.iter().map(|(text, _)| text.len()).sum::<usize>() <= EXCERPT_COLUMNS * 4 + 3);
}

#[test]
fn raster_aggregates_empty_density_and_indentation_before_downsampling() {
    let mut doc = document("notes.txt", b" x\n\n   xxx\n\n");
    std::sync::Arc::get_mut(&mut doc).unwrap().columns = COLUMNS;
    let cancelled = AtomicBool::new(false);
    let image = render(&doc, 2, 96, [[200; 3]; 9], &cancelled).unwrap();
    let alpha = |x: usize, y: usize| image.premultiplied_rgba[(y * 2 + x) * 4 + 3];
    assert_eq!(alpha(0, 0), 0);
    assert_eq!(alpha(0, 1), 127);
    assert_eq!(alpha(0, 2), 0);
    assert_eq!(alpha(1, 2), 0);
    assert_eq!(alpha(1, 3), 127);
    assert_eq!(alpha(1, 5), 127);
    assert_eq!(doc.display_range(0.0, 2), 0..2);
    assert_eq!(doc.display_range(1.0, 2), 2..4);
    assert_eq!(
        image.premultiplied_rgba,
        render(&doc, 2, 96, [[200; 3]; 9], &cancelled)
            .unwrap()
            .premultiplied_rgba
    );
}

#[test]
fn sparse_documents_expand_strokes_with_two_pixel_gaps() {
    let doc = document("notes.txt", b"example\nexample\n");
    let image = render(&doc, 300, 64, [[200; 3]; 9], &AtomicBool::new(false)).unwrap();
    let occupied = image.premultiplied_rgba[..300 * 4]
        .chunks_exact(4)
        .filter(|pixel| pixel[3] > 0)
        .count();
    assert_eq!(
        occupied, 296,
        "two source lines should each fill their slot except two gap pixels"
    );
}

#[test]
fn raster_uses_document_width_to_fit_strokes_to_available_height() {
    let doc = document("notes.txt", b"short\n              end\n");
    assert_eq!(doc.columns, 17);
    let image = render(&doc, 2, 68, [[200; 3]; 9], &AtomicBool::new(false)).unwrap();
    let alpha = |x: usize, y: usize| image.premultiplied_rgba[(y * 2 + x) * 4 + 3];
    assert_eq!(alpha(0, 0), 255);
    assert_eq!(alpha(0, 19), 255);
    assert_eq!(alpha(0, 20), 0);
    assert_eq!(alpha(1, 55), 0);
    assert_eq!(alpha(1, 56), 255);
    assert_eq!(alpha(1, 67), 255);
}

#[test]
fn million_line_file_keeps_whole_document_with_bounded_retained_data() {
    let folder = tempfile::tempdir().unwrap();
    let path = folder.path().join("million.txt");
    let mut writer = std::io::BufWriter::new(std::fs::File::create(&path).unwrap());
    for _ in 0..1000 {
        writer.write_all("short\n".repeat(1000).as_bytes()).unwrap();
    }
    writer
        .write_all(b"                  DISTINCTIVE FINAL REGION\n")
        .unwrap();
    writer.flush().unwrap();
    let start = std::time::Instant::now();
    let doc = load(&path, &AtomicBool::new(false)).unwrap();
    assert_eq!(doc.lines, 1_000_001);
    assert_eq!(doc.bins.len(), BINS);
    assert_eq!(doc.bins.last().unwrap().end, doc.lines);
    assert!(doc.bins.last().unwrap().cells[25].iter().sum::<u32>() > 0);
    assert!(doc.bins.iter().map(|bin| bin.excerpt.len()).sum::<usize>() <= BINS * EXCERPT_LINES);
    let raster_start = std::time::Instant::now();
    let image = render(&doc, 1000, 66, [[200; 3]; 9], &AtomicBool::new(false)).unwrap();
    eprintln!(
        "synthetic million-line preview: analysis={:?}, raster={:?}, retained_cells={} bytes, raster={} bytes",
        raster_start.duration_since(start),
        raster_start.elapsed(),
        BINS * COLUMNS * COLORS.len() * std::mem::size_of::<u32>(),
        image.premultiplied_rgba.len()
    );
}
