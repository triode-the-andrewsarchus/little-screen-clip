#!/usr/bin/env python3
"""Guarded document content search and embeddings on the private compositor."""
from pathlib import Path
import sqlite3
import zipfile

from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance
from file_manager_reuse_acceptance import inventory
from file_manager_search_acceptance import submit


def write_package(path, parts):
    with zipfile.ZipFile(path, "w", compression=zipfile.ZIP_DEFLATED) as archive:
        for name, text in parts.items():
            archive.writestr(name, text)


def exercise(session, root):
    session.observe("initial", ["Annual.docx"])
    index = Path(session.env["XDG_DATA_HOME"]) / "little-screen-clip/search/index.sqlite"
    assert not index.exists()
    submit(session, ':content:"violet lighthouse"')
    session.observe("document-content", ["7 matching entries", "Annual.docx"])
    assert not index.exists(), "Literal document search must not create a semantic index"
    session.key("Escape")
    submit(session, ':content:"external secret must not appear"')
    session.observe("entities-not-resolved", ["0 matching entries", "unsupported/incomplete reads"])
    session.key("Escape")
    (root / "Unsafe.docx").rename(session.output / "Unsafe.docx")
    session.observe("before-enrollment", ["Annual.docx"])
    session.click(993, 118)
    session.key("Escape")
    _, paths, vectors = inventory(index)
    assert len(paths) == 8 and vectors == 7, (paths, vectors)
    with sqlite3.connect(index) as db:
        locations = [location for location, in db.execute("SELECT location FROM segments")]
        text = "\n".join(text for text, in db.execute("SELECT text FROM segments"))
        assert any(location.startswith("slide 1") for location in locations), locations
        assert any(location.startswith("sheet Data") for location in locations), locations
        assert "external secret must not appear" not in text
        assert "network-script-marker" not in text
        assert "violet lighthouse" in text
        assert db.execute("PRAGMA user_version").fetchone()[0] == 5
    submit(session, ':emb:"violet lighthouse on a rocky coast"')
    session.observe("document-semantic", ["7 matching entries", "Semantic candidates", "Annual.docx"])


def main():
    repository = Path(__file__).resolve().parent.parent
    args = application_arguments(__doc__, repository / "target/debug/clip-file-manager")
    output, root = media_fixture_directory(args)
    phrase = "The violet lighthouse stands on a rocky coast."
    write_package(root / "Annual.docx", {"word/document.xml": f'<w:document xmlns:w="word"><w:p><w:r><w:t>{phrase}</w:t></w:r></w:p></w:document>'})
    write_package(root / "Notes.odt", {"content.xml": f'<document xmlns:t="text"><t:p>{phrase}</t:p></document>'})
    write_package(root / "Slides.pptx", {
        "ppt/presentation.xml": '<presentation xmlns:r="relations"><sldId r:id="slide"/></presentation>',
        "ppt/_rels/presentation.xml.rels": '<Relationships><Relationship Id="slide" Target="slides/slide1.xml"/></Relationships>',
        "ppt/slides/slide1.xml": f'<slide><p><t>{phrase}</t></p></slide>',
    })
    write_package(root / "Workbook.xlsx", {
        "xl/workbook.xml": '<workbook xmlns:r="relations"><sheets><sheet name="Data" r:id="data"/></sheets></workbook>',
        "xl/_rels/workbook.xml.rels": '<Relationships><Relationship Id="data" Target="worksheets/sheet1.xml"/></Relationships>',
        "xl/worksheets/sheet1.xml": f'<worksheet><sheetData><row><c r="A1" t="inlineStr"><is><t>{phrase}</t></is></c></row></sheetData></worksheet>',
    })
    (root / "Page.html").write_text(f'<script>fetch("https://invalid.test/network-script-marker")</script><p>{phrase}</p>')
    (root / "Unicode16.txt").write_bytes(phrase.encode("utf-16"))
    (root / "Unicode32.txt").write_bytes(phrase.encode("utf-32"))
    (root / "Source.rs").write_text(phrase)  # Code remains name-only.
    secret = output / "external-secret.txt"
    secret.write_text("external secret must not appear")
    write_package(root / "Unsafe.docx", {"word/document.xml": f'<!DOCTYPE x [<!ENTITY secret SYSTEM "{secret.as_uri()}">]><x><p><t>&secret;</t></p></x>'})

    def environment(env, children):
        data = Path(env["XDG_DATA_HOME"]) / "little-screen-clip/search"
        data.mkdir(parents=True)
        for directory in ["runtime", "models"]:
            (data / directory).symlink_to(Path.home() / ".local/share/little-screen-clip/search" / directory, target_is_directory=True)

    return run_application_acceptance(args, output, root, exercise,
        "DOCX/ODT/PPTX/XLSX/HTML and UTF-16/32 literal search, passive XML admission and real embeddings without code indexing", environment)


if __name__ == "__main__":
    raise SystemExit(main())
