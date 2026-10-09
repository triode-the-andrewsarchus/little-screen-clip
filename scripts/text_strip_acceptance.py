#!/usr/bin/env python3
"""Exercise inline text strips on the private Weston/Niri desktop only."""
from contextlib import contextmanager
from itertools import groupby
from pathlib import Path
import subprocess
import time

from PIL import Image
import ui_acceptance_capture as private
from file_manager_acceptance import (
    application_arguments, media_fixture_directory, run_application_acceptance,
    preserve_file_clipboard, wait_for,
)


def fixtures(root):
    source = []
    for block in range(25):
        region = "BEGIN_REGION" if block < 5 else "END_REGION" if block >= 20 else "MIDDLE_REGION"
        source += [f"// Region {region}", f"fn preview_{block:03}() {{",
                   f'    let marker = "{region}";', '    println!("{}", marker);', "}", ""]
    (root / "a-preview.rs").write_text("\n".join(source) + "\n")
    (root / "b-config.toml").write_text('[section]\nname = "Example"\nenabled = true\n\n' * 30)
    (root / "c-layout.kdl").write_text('// comment\nnode name="Example" {\n    child #true\n}\n' * 30)
    (root / "d-type.ts").write_text('interface Example { value: string; }\nconst name = "Example";\n' * 30)
    (root / "e-notes.unknown").write_text("Readable notes from the entire file.\n" * 80)
    (root / "f-binary.bin").write_bytes(b"binary\0not text")
    (root / "g-empty.txt").write_bytes(b"")
    for index in range(20):
        (root / f"t-row-{index:02}.txt").write_text(f"Visible scrolling row {index}\n" * 20)


def first_strip(path):
    with Image.open(path) as image:
        pixels = image.convert("RGB").load()
        rows = []
        for y in range(120, min(image.height, 700)):
            count = sum(max(pixels[x, y]) - min(pixels[x, y]) > 40
                        for x in range(350, image.width - 60))
            rows.append((y, count))
        bands = [list(group) for colored, group in
                 groupby(rows, lambda pair: pair[1] > 50) if colored]
        bands = [band for band in bands if len(band) >= 12]
        if not bands:
            return None
        band = bands[0]
        points = [(x, y) for y, _ in band for x in range(350, image.width - 60)
                  if max(pixels[x, y]) - min(pixels[x, y]) > 40]
        return min(x for x, _ in points), max(x for x, _ in points), band[len(band) // 2][0]


@contextmanager
def hover(session, x, y):
    process = subprocess.Popen([str(session.pointer), "move", "1280", "800", str(x), str(y)],
                               env=session.env, stdout=subprocess.DEVNULL)
    try:
        yield
    finally:
        process.terminate()
        process.wait(timeout=3)


def hover_observation(session, name, x, y, expected):
    path = session.output / f"{name}.png"
    with hover(session, x, y):
        text = None
        def ready():
            nonlocal text
            text = session.capture_text(name, path, expected)
            visible = "".join(text.lower().split())
            return all("".join(fragment.lower().split()) in visible for fragment in expected)
        wait_for(ready, name)
    (session.output / f"{name}.txt").write_text(text)
    session.observations.append({"name": name, "screenshot": str(path),
                                 "expected_visible_text": expected, "hover": [x, y]})


def exercise(session, root):
    session.observe("01-inline", ["a-preview.rs", "150 lines", "b-config.toml", "Modified"])
    strip = None
    def painted():
        nonlocal strip
        with preserve_file_clipboard(session.env):
            private.screenshot(session.niri, session.env, session.output / "02-painted.png")
        strip = first_strip(session.output / "02-painted.png")
        return strip is not None
    wait_for(painted, "syntax-colored horizontal strip")
    left, right, y = strip
    hover_observation(session, "03-hover-begin", round(left + (right - left) * .08), y,
                      ["Rust", "BEGIN_REGION"])
    hover_observation(session, "04-hover-end", round(left + (right - left) * .9), y,
                      ["Rust", "END_REGION"])
    session.click(round((left + right) / 2), y)
    session.key("c", "ctrl")
    copied = subprocess.run(["wl-paste", "--no-newline", "--type", "text/uri-list"],
                            env=session.env, capture_output=True, timeout=5, check=True).stdout
    assert (root / "a-preview.rs").as_uri().encode() in copied, "minimap click did not select its file"
    session.key("Return")
    session.observe("05-existing-text-viewer", ["a-preview.rs", "Preview", "Edit"])
    session.key("Escape")
    session.observe("06-return", ["a-preview.rs", "150 lines"])
    private.run_checked([str(session.pointer), "scroll", "1280", "800",
                         str(round((left + right) / 2)), str(y), "600"], env=session.env)
    session.observe("07-wheel-over-minimap", ["d-type.ts", "60 lines"])
    x, title_y = session.word("07-wheel-over-minimap", "d-type.ts")
    session.click(x + 20, title_y)
    session.key("Return")
    session.observe("07b-typescript-viewer", ["d-type.ts", "Preview", "Edit"])
    session.key("Escape")
    session.click(x + 20, title_y)
    for _ in range(8):
        session.key("Up")
    session.observe("08-reset", ["a-preview.rs"])
    # Resizing must retain the analysis and repaint a dimension-specific strip.
    private.run_checked([str(session.niri), "msg", "action", "set-window-width", "850"], env=session.env)
    session.observe("09-resized", ["a-preview.rs", "150 lines"])
    time.sleep(.2)
    with preserve_file_clipboard(session.env):
        private.screenshot(session.niri, session.env, session.output / "10-resized-painted.png")
    assert first_strip(session.output / "10-resized-painted.png"), "resized minimap disappeared"


def main():
    args = application_arguments(__doc__, Path("target/release/clip-file-manager"))
    output, root = media_fixture_directory(args)
    fixtures(root)
    return run_application_acceptance(args, output, root, exercise, "inline text minimaps")


if __name__ == "__main__":
    raise SystemExit(main())
