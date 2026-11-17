#!/usr/bin/env python3
"""Exercise both neural Deskew options on the private Weston/Niri runner."""

import argparse
import csv
import io
import json
from pathlib import Path
import subprocess
import time

from PIL import Image, ImageChops, ImageFilter

from file_manager_acceptance import Session, preserve_file_clipboard
import ui_acceptance_capture as private


def primary_control_regions(screen):
    """Find filled controls in the observed sidebar of the fixed private output."""
    pixels = screen.convert("RGB").load()
    def blue(x, y):
        r, g, b = pixels[x, y]
        return b - r > 60 and g - r > 60
    regions, start = [], None
    for y in range(365, 717):
        if blue(65, y):
            if start is None:
                start = y
        elif start is not None:
            if y - start >= 12:
                middle = (start + y) // 2
                xs = [x for x in range(60, 290) if blue(x, middle)]
                if xs and max(xs) - min(xs) > 60:
                    regions.append((f"primary-{start}", (min(xs) + 8, start + 5, max(xs) - 7, y - 5)))
            start = None
    return regions


class DeskewSession(Session):
    """Read this studio's sidebar and footer, including reversed button text."""

    def move_pointer_to_title(self):
        # Filename hover cards cover the sidebar; park on empty canvas instead.
        pointer = subprocess.Popen([str(self.pointer), "move", "1280", "800", "1100", "600"], env=self.env, stdout=subprocess.DEVNULL)
        time.sleep(0.1)
        pointer.terminate()
        pointer.wait(timeout=3)

    def capture_text(self, name, path, expected):
        path.unlink(missing_ok=True)
        with preserve_file_clipboard(self.env):
            private.screenshot(self.niri, self.env, path)
        words, texts = [], []
        with Image.open(path) as screen:
            regions = [
                ("panel", (52, 193, 300, 727)),
                ("footer", (52, 727, 1230, 772)),
                ("selector-status", (950, 733, 1197, 767)),
                # Text interiors exclude icons/borders that confuse OCR.
                ("selector-manual", (90, 232, 220, 257)),
                ("selector-uvdoc", (90, 274, 143, 300)),
                ("selector-synthetic", (90, 316, 190, 342)),
                ("selector-reset", (88, 664, 178, 692)),
            ]
            regions += primary_control_regions(screen)
            if expected in (["Undone"], ["Redone"]):
                # Poll transient history messages quickly; scanning every control
                # can consume the whole lifetime of the toast under build load.
                regions = [(label, bounds) for label, bounds in regions
                           if label in ("footer", "selector-status")]
            for region, bounds in regions:
                image = screen.crop(bounds).convert("RGB")
                pixels = list(image.get_flattened_data())
                bright = Image.new("L", image.size)
                cyan = Image.new("L", image.size)
                bright.putdata([255 if max(p) - min(p) < 25 and max(p) > 140 else 0 for p in pixels])
                cyan.putdata([255 if max(p) - min(p) > 60 and p[2] > p[0] else 0 for p in pixels])
                dark = image.convert("L").point(lambda value: 255 if value < 90 else 0)
                masks = [ImageChops.invert(bright), ImageChops.invert(ImageChops.lighter(bright, ImageChops.multiply(dark, cyan.filter(ImageFilter.MaxFilter(7)))))]
                # Light themes already provide dark text on a light surface.
                masks.append(image.convert("L"))
                for index, mask in enumerate(masks):
                    ocr = self.output / f"{name}-{region}-{index}.png"
                    mask.resize((mask.width * 3, mask.height * 3)).save(ocr)
                    mode = "7" if region.startswith(("selector-", "primary-")) else "6"
                    data = private.run_checked(["tesseract", str(ocr), "stdout", "--psm", mode, "-c", "tessedit_create_tsv=1", "-c", "tessedit_create_txt=0"], env=self.env, timeout=15)
                    current = []
                    for word in csv.DictReader(io.StringIO(data), delimiter="\t", quoting=csv.QUOTE_NONE):
                        text = word["text"].strip()
                        if text:
                            current.append(text)
                            words.append((text.lower().strip("()[]{}|"), bounds[0] + (int(word["left"]) + int(word["width"]) // 2) // 3, bounds[1] + (int(word["top"]) + int(word["height"]) // 2) // 3))
                    texts.append(" ".join(current))
        if not hasattr(self, "control_words"):
            self.control_words = {}
        self.control_words[name] = words
        return "\n".join(texts)

    def word(self, name, text, tree=False, required=True, **kwargs):
        matches = [(x, y) for word, x, y in self.control_words[name] if word == text.lower() and (x < 330) == tree]
        if matches:
            return matches[0]
        if required:
            raise RuntimeError(f"{name}: no control {text!r}")
        return None


def click_control(session, shot, word):
    point = session.word(shot, word, tree=True, required=False) or session.word(shot, word)
    session.click(*point)


def exercise(session):
    session.key("k")
    session.observe("01-manual", ["Deskew method", "Manual", "UVDoc", "SyntheticDoc", "Click 4 Points"])
    previous = "01-manual"
    for number, method in enumerate(["UVDoc", "SyntheticDoc"], 2):
        selected = f"{number:02}-{method}-selected"
        click_control(session, previous, method)
        expected = ["Preview Unwarp", "Runs locally"]
        if method == "SyntheticDoc":
            expected.append("Correct illumination")
        session.observe(selected, expected)
        if method == "UVDoc":
            click_control(session, selected, "Preview")
            session.key("Escape")
            cancelled = session.observe("02-cancelled", ["Preview Unwarp", "Runs locally"])
            assert "Flattening with" not in cancelled and "Apply Unwarp" not in cancelled
        else:
            click_control(session, selected, "illumination")
        click_control(session, selected, "Preview")
        preview = f"{number:02}-{method}-preview"
        session.observe(preview, ["Apply Unwarp", "Show original"], timeout_seconds=60)
        click_control(session, preview, "original")
        original = f"{number:02}-{method}-original"
        session.observe(original, ["Apply Unwarp", "Show original"])
        click_control(session, original, "original")
        session.key("Return")
        applied = f"{number:02}-{method}-applied"
        session.observe(applied, [f"Applied {method}", "Preview Unwarp"])
        session.key("z", "ctrl")
        session.observe(f"{number:02}-{method}-undo", ["Undone"])
        session.key("y", "ctrl")
        session.observe(f"{number:02}-{method}-redo", ["Redone"])
        session.key("z", "ctrl")
        previous = f"{number:02}-{method}-restored"
        session.observe(previous, ["Undone", "Preview Unwarp"])
    click_control(session, previous, "Manual")
    session.observe("04-manual-return", ["Click 4 Points", "Reset Quad"])
    session.key("Return")
    session.observe("05-manual-applied", ["Deskewed perspective"])
    session.key("z", "ctrl")
    session.observe("06-manual-undo", ["Undone", "Click 4 Points"])


def main():
    repo = Path(__file__).resolve().parent.parent
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output-dir", type=Path, required=True)
    parser.add_argument("--appearance", choices=["light", "dark"], default="dark")
    parser.add_argument("--binary", type=Path, default=repo / "target/debug/clip-workbench")
    parser.add_argument("--image", type=Path, default=repo / "crates/clip-studios/assets/document-unwarp/test/uvdoc-input.png")
    args = parser.parse_args()
    output = args.output_dir.resolve()
    output.mkdir(parents=True, exist_ok=False)
    private_args = private.create_parser().parse_args([
        "--output-dir", str(output), "--appearance", args.appearance,
        "--image", str(args.image.resolve()), "--allow-server-decorations",
    ])
    children = private.ChildProcesses()
    report = {"status": "running", "appearance": args.appearance}
    try:
        config = private.prepare_configuration(private_args, output)
        _, env, _ = private.start_compositors(config, private_args, output, children)
        process = children.start("workbench", private.app_arguments(private_args, args.binary.resolve()),
                                 env=env, cwd=config["scratch"], log_path=output / "workbench.log")
        windows, _ = private.wait_for_app_window(config["niri_bin"], env, process, output)
        private.run_checked([str(config["niri_bin"]), "msg", "action", "focus-window", "--id", str(windows[0]["id"])], env=env)
        session = DeskewSession(output, env, config["niri_bin"], process, children)
        exercise(session)
        report |= {"status": "passed", "observations": session.observations}
    except Exception as error:
        report |= {"status": "failed", "error": str(error)}
    finally:
        children.cleanup()
        (output / "report.json").write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps(report))
    return 0 if report["status"] == "passed" else 1


if __name__ == "__main__":
    raise SystemExit(main())
