#!/usr/bin/env python3
"""Review guarded expanded-folder search on the private native compositor."""
from pathlib import Path
import os
import subprocess
import zipfile
from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance, wait_for
from file_manager_search_acceptance import submit


def fixtures(root):
    (root / "a-open/nested").mkdir(parents=True)
    (root / "b-closed").mkdir()
    (root / "c-links").mkdir()
    (root / "root.txt").write_text("orchid sentinel at the root\n")
    (root / "a-open/direct.txt").write_text("orchid sentinel in an expanded folder\n")
    (root / "a-open/nested/deep.txt").write_text("orchid sentinel in the nested folder\n")
    for i in range(10):
        (root / "b-closed" / f"closed-{i:02}.txt").write_text("orchid sentinel outside the explicit expansion\n")
    outside = root.parent / "outside-data"
    outside.mkdir()
    (outside / "outside.txt").write_text("orchid sentinel outside the browser scope\n")
    os.symlink(outside, root / "c-links/outside")
    with zipfile.ZipFile(root / "d-archive.zip", "w") as archive:
        archive.writestr("member.txt", "orchid sentinel is an archive member")


def exercise(session, root):
    session.ocr_regions = [("strip-title-deep.png", (430, 410, 545, 432))]
    session.observe("initial", ["a-open", "b-closed"])
    session.click(645, 160)  # Collapse Shelf in the observed initial List toolbar.
    session.observe("loaded-folder", ["direct.txt", "nested"])
    session.click(510, 320)  # Separate disclosure on the nested-folder card.
    session.observe("expanded-tree", ["deep.txt", "a-open"])
    submit(session, ':file;regex:"(direct|deep|closed|outside|member)";scope:here')
    session.observe("name-results", ["Folder + expanded", "2 matching entries", "deep.txt", "direct.txt"])
    submit(session, ':content:"orchid sentinel";scope:here')
    session.observe("content-results", ["Folder + expanded", "3 matching entries", "root.txt", "deep.txt"])
    submit(session, ':file;emb:"deep";scope:here')
    session.observe("no-index-fallback", ["Filename results only", "Folder + expanded", "1 matching entries", "deep.txt"])
    x, y = session.word("no-index-fallback", "deep.txt", minimum_y=190)
    session.click(x + 3, y)
    session.key("c", "ctrl")
    def copied():
        result = subprocess.run(["wl-paste", "--no-newline", "--type", "text/uri-list"],
            env=session.env, capture_output=True, timeout=5)
        return result.returncode == 0 and (root / "a-open/nested/deep.txt").as_uri().encode() in result.stdout
    wait_for(copied, "guarded search-result clipboard URI")
    # Preserve the exact leaf and immediate-parent identities while replacing
    # an observed ancestor. A broad-root-only guard would wrongly accept this.
    before = (root / "a-open/nested/deep.txt").stat()
    (root / "a-open").rename(root / "retired-open")
    (root / "a-open").mkdir()
    (root / "retired-open/nested").rename(root / "a-open/nested")
    after = (root / "a-open/nested/deep.txt").stat()
    assert (before.st_dev, before.st_ino, before.st_mtime_ns, before.st_ctime_ns) == (after.st_dev, after.st_ino, after.st_mtime_ns, after.st_ctime_ns)
    session.key("Return")
    session.observe("ancestor-replacement-rejected", ["Back to folder", "Search result changed"])
    assert (root / "a-open/nested/deep.txt").read_text() == "orchid sentinel in the nested folder\n"
    assert not (root / "member.txt").exists()
    session.key("q", "ctrl")  # Return from the refused studio open.


if __name__ == "__main__":
    args = application_arguments(__doc__, Path("target/debug/clip-file-manager"))
    output, root = media_fixture_directory(args)
    fixtures(root)
    raise SystemExit(run_application_acceptance(args, output, root, exercise,
        "Expanded here names/content/unenrolled fallback, guarded child copy, closed-folder/alias/archive separation and ancestor lifetime checks"))
