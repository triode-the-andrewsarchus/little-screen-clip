#!/usr/bin/env python3
"""Real tokenizer passage persistence and overlap on the private compositor."""
from pathlib import Path
import re
import sqlite3

from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance
from file_manager_reuse_acceptance import inventory
from file_manager_search_acceptance import submit


def exercise(session, root):
    session.observe("initial", ["Long.txt", "Unicode.txt"])
    index = Path(session.env["XDG_DATA_HOME"]) / "little-screen-clip/search/index.sqlite"
    assert not index.exists()
    session.click(993, 118)
    session.key("Escape")
    _, paths, vectors = inventory(index)
    assert len(paths) == 2 and vectors > 4, (paths, vectors)
    evidence = []
    with sqlite3.connect(index) as db:
        for path in sorted(root.iterdir()):
            source = path.read_bytes()
            segments = db.execute("SELECT s.location,s.text FROM segments s JOIN paths p ON p.file_id=s.file_id WHERE p.path=? ORDER BY s.id", (bytes(path),)).fetchall()
            covered, overlaps = 0, 0
            for location, text in segments:
                match = re.search(r"UTF-8 bytes (\d+)\.\.(\d+)$", location)
                assert match, location
                start, end = map(int, match.groups())
                assert source[start:end].decode() == text
                assert start <= covered < end, (path, covered, start, end)
                assert len(text.encode()) <= 8192
                overlaps += int(start < covered)
                covered = end
            assert covered == len(source) and overlaps > 0, (path, covered, len(source), overlaps)
            evidence.append({"name": path.name, "segments": len(segments), "bytes": covered, "overlaps": overlaps})
        assert db.execute("SELECT MIN(extraction_version) FROM files").fetchone()[0] == 4
    import json
    (session.output / "passages.json").write_text(json.dumps(evidence, indent=2))
    submit(session, ':emb:"violet lighthouse"')
    session.observe("long-document-search", ["Long.txt", "Semantic candidates"])


def main():
    repository = Path(__file__).resolve().parent.parent
    args = application_arguments(__doc__, repository / "target/debug/clip-file-manager")
    output, root = media_fixture_directory(args)
    (root / "Long.txt").write_text("\n\n".join(f"Paragraph {i}. The violet lighthouse stands above a rocky coast. Its lamp guides the fishing boats safely home. " * 5 for i in range(20)))
    (root / "Unicode.txt").write_text("\n\n".join(f"Passage {i}. 音楽と海の物語。 किताब पढ़ें। A café beside the harbour. 🎵 " * 5 for i in range(20)))

    def environment(env, children):
        data = Path(env["XDG_DATA_HOME"]) / "little-screen-clip/search"
        data.mkdir(parents=True)
        for directory in ["runtime", "models"]:
            (data / directory).symlink_to(Path.home() / ".local/share/little-screen-clip/search" / directory, target_is_directory=True)

    return run_application_acceptance(args, output, root, exercise,
        "Real multilingual tokenizer chunks, complete byte coverage, overlap, persisted locations and semantic search", environment)


if __name__ == "__main__":
    raise SystemExit(main())
