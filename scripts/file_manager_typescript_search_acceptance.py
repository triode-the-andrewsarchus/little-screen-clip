#!/usr/bin/env python3
"""Pinned TypeScript/transport-stream classification on the private compositor."""
from pathlib import Path
import json
import sqlite3
import subprocess

from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance
from file_manager_reuse_acceptance import inventory, link_installed_inference
from file_manager_search_acceptance import submit


def exercise(session, root):
    session.observe("initial", ["Legacy.rtf", "module.cts"])
    session.click(647, 160)  # Collapse Shelf so four rich source rows fit.
    index = Path(session.env["XDG_DATA_HOME"]) / "little-screen-clip/search/index.sqlite"
    submit(session, ':document;content:"violet lighthouse";scope:recursive')
    session.observe("literal-sources", ["4 matching entries", "module.mts", "module.cts", "Note.rst"])
    session.scroll_to_visible("literal-source-last-row", ["module.ts"], position=(1050, 450))
    session.key("Escape")
    submit(session, ':video,*.ts;scope:recursive')
    session.observe("transport-stream", ["1 matching entries", "recording.ts"])
    session.key("Escape")
    assert not index.exists(), "Literal and type filters must not enroll sources"
    assert not index.parent.joinpath("runtime").exists()
    assert not index.parent.joinpath("models").exists()
    # Keep this enrollment check focused on source/video dispatch.
    # The document acceptance scenario covers RTF extraction separately.
    (root / "Legacy.rtf").rename(session.output / "Legacy.rtf")
    link_installed_inference(index)
    session.click(993, 118)
    session.key("Escape")
    _, paths, vectors = inventory(index)
    assert len(paths) == 5 and vectors == 5, (paths, vectors)
    with sqlite3.connect(index) as db:
        rows = db.execute("SELECT p.path,f.metadata,f.state,s.modality,s.text FROM paths p JOIN files f ON f.id=p.file_id LEFT JOIN segments s ON s.file_id=f.id").fetchall()
    records = []
    for path, metadata, state, modality, text in rows:
        name = Path(bytes(path).decode()).name
        script = name.startswith("module.")
        assert json.loads(metadata)["typescript"] == script, (name, metadata)
        if script or name == "Note.rst":
            assert modality == "text" and "violet lighthouse" in text, (name, modality, text)
        elif name == "recording.ts":
            assert modality == "image", (name, modality)
        else:
            raise AssertionError((name, state, modality))
        records.append(dict(name=name, state=state, modality=modality, typescript=script))
    (session.output / "indexed-formats.json").write_text(json.dumps(records, indent=2))
    submit(session, ':document;emb:"violet lighthouse";scope:recursive')
    session.observe("semantic-sources", ["4 matching entries", "Semantic candidates"])
    session.scroll_to_visible("semantic-source-last-row", ["module.ts"], position=(1050, 450))


def main():
    repository = Path(__file__).resolve().parent.parent
    args = application_arguments(__doc__, repository / "target/release/clip-file-manager")
    output, root = media_fixture_directory(args)
    for suffix in ["ts", "mts", "cts"]:
        (root / f"module.{suffix}").write_text('export const sentinel = "violet lighthouse";\n')
    (root / "Note.rst").write_text("The violet lighthouse stands on a rocky coast.\n")
    (root / "Legacy.rtf").write_text(r"{\rtf1\ansi An unrelated archived letter.}")
    subprocess.run(["ffmpeg", "-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i", "color=c=blue:s=96x64:r=25:d=1", "-an", "-c:v", "mpeg2video", "-muxrate", "1000000", "-f", "mpegts", str(root / "recording.ts")], check=True, timeout=20)
    raise SystemExit(run_application_acceptance(args, output, root, exercise, "pinned TypeScript and binary transport-stream classification; no implicit enrollment; full indexing dispatch"))


if __name__ == "__main__":
    main()
