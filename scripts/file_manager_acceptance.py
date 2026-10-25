#!/usr/bin/env python3
"""Exercise the standalone file manager on the existing private Weston/Niri runner.

All keyboard/pointer input and screenshots target this run's private sockets.
Uses synthetic files and leaves the user's compositor, daemon, and clipboard alone.
"""

import argparse
from contextlib import contextmanager
import csv
import io
import json
import time
import subprocess
from pathlib import Path

from PIL import Image

import ui_acceptance_capture as private


@contextmanager
def preserve_file_clipboard(env):
    """Niri's capture action publishes a PNG; retain references and plain text under test."""
    types = subprocess.run(["wl-paste", "--list-types"], env=env, capture_output=True, timeout=5).stdout.decode(errors="replace").splitlines()
    mime = next((kind for kind in ["x-special/gnome-copied-files", "text/uri-list", "text/plain;charset=utf-8", "text/plain", "UTF8_STRING"] if kind in types), None)
    payload = None
    if mime:
        result = subprocess.run(["wl-paste", "--no-newline", "--type", mime], env=env, capture_output=True, timeout=5)
        if result.returncode == 0:
            payload = result.stdout
    try:
        yield
    finally:
        if payload is not None:
            subprocess.run(["wl-copy", "--type", mime], input=payload, env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=5, check=True)


class Session:
    def __init__(self, output, env, niri, process):
        self.output, self.env, self.niri, self.process = output, env, niri, process
        self.observations = []
        self.pointer = private.build_private_virtual_pointer(output)
        private.verify_private_wayland_target(env, output / "runtime")
        private.run_checked([str(self.pointer), "probe"], env=env)

    def key(self, key, modifier=None):
        command = ["wtype"]
        if modifier:
            command += ["-M", modifier]
        command += ["-k", key]
        if modifier:
            command += ["-m", modifier]
        private.run_checked(command, env=self.env)
        time.sleep(0.15)

    def address(self, path):
        self.key("l", "ctrl")
        private.run_checked(["wtype", str(path)], env=self.env)
        self.key("Return")

    def click(self, x, y, delay=0.2):
        private.run_checked([str(self.pointer), "click", "1280", "800", str(x), str(y)], env=self.env)
        time.sleep(delay)

    def move_pointer_to_title(self):
        pointer = subprocess.Popen([str(self.pointer), "move", "1280", "800", "120", "70"], env=self.env, stdout=subprocess.DEVNULL)
        time.sleep(0.1)
        pointer.terminate()
        pointer.wait(timeout=3)

    def observe(self, name, expected, expected_any=(), preserve_pointer=False):
        if not preserve_pointer:
            self.move_pointer_to_title()
        # Poll rendered output, not a guessed fixed delay, until the requested
        # visible evidence appears. Keep the PNG and extracted text for review.
        path = self.output / f"{name}.png"
        text = ""
        def visible():
            nonlocal text
            if self.process.poll() is not None:
                raise RuntimeError("file manager exited during acceptance")
            text = self.capture_text(name, path, expected)
            rendered = "".join(text.lower().split())
            required = all("".join(fragment.lower().split()) in rendered for fragment in expected)
            alternatives = not expected_any or any("".join(fragment.lower().split()) in rendered for fragment in expected_any)
            return required and alternatives
        wait_for(visible, f"{name}: {expected}; any of {expected_any}")
        (self.output / f"{name}.txt").write_text(text)
        self.observations.append({"name": name, "screenshot": str(path), "expected_visible_text": expected, "expected_any": list(expected_any)})
        return text

    def scroll_to_visible(self, name, expected, step=150, attempts=24, position=(1050, 600)):
        """Find inspector evidence with bounded scrolling on the private pointer."""
        for index in range(attempts):
            self.move_pointer_to_title()
            scan = f"{name}-scan-{index}"
            text = self.capture_text(scan, self.output / f"{scan}.png", expected)
            rendered = "".join(text.lower().split())
            if all("".join(fragment.lower().split()) in rendered for fragment in expected):
                self.observe(name, expected)
                return
            private.run_checked([
                str(self.pointer), "scroll", "1280", "800", *map(str, position), str(step),
            ], env=self.env)
            time.sleep(0.5)
        raise RuntimeError(f"Unable to find {expected} in bounded inspector scroll")

    def capture_text(self, name, path, expected):
        # Niri writes asynchronously; an existing file must not satisfy readiness.
        path.unlink(missing_ok=True)
        with preserve_file_clipboard(self.env):
            private.screenshot(self.niri, self.env, path)
        ocr_path = self.output / f"{name}-ocr.png"
        with Image.open(path) as screen:
            screen.resize((screen.width * 3, screen.height * 3)).save(ocr_path)
        text = private.run_checked(["tesseract", str(ocr_path), "stdout", "--psm", "11"], env=self.env)
        visible = "".join(text.lower().split())
        if all("".join(fragment.lower().split()) in visible for fragment in expected):
            return text
        # Sparse studios can omit the toolbar in full-window OCR. Inspect the
        # listing and toolbar regions separately while retaining the raw PNG.
        regions = [
            ("listing-ocr.png", (340, 280, 1180, 710)),
            ("toolbar-ocr.png", (90, 90, 1180, 280)),
            ("player-ocr.png", (90, 610, 1180, 775)),
            ("dialog-ocr.png", (320, 330, 960, 540)),
            ("pad-mixer-ocr.png", (350, 518, 464, 718)),
            ("pad-retrigger-ocr.png", (419, 685, 488, 704)),
            ("pad-pan-ocr.png", (512, 685, 542, 704)),
            ("pad-strip-ocr.png", (420, 660, 790, 730)),
            ("pad-confirm-ocr.png", (494, 478, 584, 497)),
        ]
        if "High" in expected:
            title = self.word(name, "priority", required=False)
            if title:
                x, y = title
                regions.insert(0, ("pad-menu-ocr.png", (x - 48, y + 20, x + 47, y + 140)))
        for filename, bounds in regions:
            region = self.output / filename
            with Image.open(path) as screen:
                crop = screen.crop(bounds)
                crop.resize((crop.width * 3, crop.height * 3)).save(region)
            text += private.run_checked(["tesseract", str(region), "stdout", "--psm", "6"], env=self.env)
            visible = "".join(text.lower().split())
            if all("".join(fragment.lower().split()) in visible for fragment in expected):
                break
        return text

    def word(self, name, text, tree=False, required=True, minimum_y=0, last=False, suffix=False):
        data = private.run_checked([
            "tesseract", str(self.output / f"{name}-ocr.png"), "stdout", "--psm", "11", "-c", "tessedit_create_tsv=1", "-c", "tessedit_create_txt=0",
        ], env=self.env)
        matches = []
        for word in csv.DictReader(io.StringIO(data), delimiter="\t", quoting=csv.QUOTE_NONE):
            rendered = word["text"].strip().lower().strip("()[]{}|")
            if (rendered != text.lower() and not (suffix and rendered.endswith("/" + text.lower()))):
                continue
            x, y = int(word["left"]) // 3, int(word["top"]) // 3
            if (x < 330) == tree and y >= minimum_y:
                matches.append((x, y + int(word["height"]) // 6))
        if matches:
            return matches[-1] if last else matches[0]
        if required:
            raise RuntimeError(f"{name}: no {text!r} in {'tree' if tree else 'listing'}")
        return None

    def trash_row(self, name, path):
        """Resolve only the full owned path, joining OCR fragments on its row."""
        data = private.run_checked([
            "tesseract", str(self.output / f"{name}-ocr.png"), "stdout", "--psm", "11",
            "-c", "tessedit_create_tsv=1", "-c", "tessedit_create_txt=0",
        ], env=self.env)
        words = [word for word in csv.DictReader(io.StringIO(data), delimiter="\t", quoting=csv.QUOTE_NONE)
                 if word['text'].strip() and 330 <= int(word['left']) // 3 < 960
                 and int(word['top']) // 3 >= 400]
        wanted = str(path)
        for word in words:
            y = (int(word['top']) + int(word['height']) / 2) / 3
            row = sorted((other for other in words
                          if abs((int(other['top']) + int(other['height']) / 2) / 3 - y) < 5),
                         key=lambda other: int(other['left']))
            rendered = ''.join(other['text'] for other in row)
            if wanted in rendered:
                self.output.joinpath('verified-trash-source.txt').write_text(str(path))
                return int(row[0]['left']) // 3 + 12, round(y)
        raise RuntimeError(f"{name}: no verified Trash row for owned fixture {path}")


def fixtures(output):
    root = output / "fixture"
    (root / "Album").mkdir(parents=True)
    (root / "Empty").mkdir()
    (root / ".hidden-folder").mkdir()
    (root / "Album" / "DOCUMENT").write_text("inside the album")
    (root / "welcome.txt").write_text("welcome")
    (root / "file with spaces.txt").write_text("spaces")
    (root / ".hidden-file").write_text("hidden")
    (root / "Album link").symlink_to(root / "Album", target_is_directory=True)
    (root / "broken-link").symlink_to(root / "missing")
    return root


def exercise(session, root):
    session.observe("01-browser", ["Files", "Folders", "Album", "6 items"])
    session.address(root / "Album")
    session.observe("02-address", ["DOCUMENT"])
    session.key("Left", "alt")
    session.observe("03-back", ["Album", "6 items"])
    session.key("Right", "alt")
    session.observe("04-forward", ["DOCUMENT"])
    session.key("Up", "alt")
    session.observe("05-parent", ["Album", "6 items"])
    session.key("h", "ctrl")
    session.observe("06-hidden", ["hidden-file", "hidden-folder"])
    session.key("h", "ctrl")
    session.address(root / "Empty")
    session.observe("07-empty", ["This folder is empty"])
    session.address(root / "does-not-exist")
    session.observe("08-error", ["Cannot open", "does-not-exist", "This folder is empty"])
    session.address(root / "Album link")
    session.observe("09-symlink", ["DOCUMENT"])
    session.key("l", "ctrl")
    private.run_checked(["wtype", "discard this address"], env=session.env)
    session.key("Escape")
    session.observe("10-address-escape", ["DOCUMENT", str(root / "Album")])
    session.address(root)
    (root / "NEW-FILE.txt").write_text("refresh must find this")
    session.key("F5")
    session.observe("11-refresh", ["NEW-FILE.txt", "7 items"])
    x, y = session.word("11-refresh", "Album")
    session.click(x + 10, y)
    session.key("Return")
    session.observe("12-list-open", ["DOCUMENT"])
    session.key("Left", "alt")
    session.observe("13-list-return", ["7 items"])
    x, y = session.word("13-list-return", "fixture", tree=True)
    session.click(x - 37, y)
    session.observe("14-tree-collapse", ["7 items"])
    if session.word("14-tree-collapse", "Album", tree=True, required=False) is not None:
        raise RuntimeError("collapsing the tree did not hide its children")
    session.click(x - 37, y)
    session.observe("15-tree-expand", ["7 items"])
    x, y = session.word("15-tree-expand", "Album", tree=True)
    session.click(x + 10, y)
    session.observe("16-tree-open", ["DOCUMENT"])
    session.key("Up")
    session.observe("17-tree-select", ["DOCUMENT"])
    session.key("Return")
    session.observe("18-tree-keyboard-open", ["7 items"])
    session.key("Left", "alt")
    session.observe("19-history-return", ["DOCUMENT"])
    session.key("Up", "alt")
    session.observe("20-double-click-source", ["7 items"])
    x, y = session.word("20-double-click-source", "Album")
    session.click(x + 10, y, delay=0.05)
    session.click(x + 10, y)
    session.observe("21-double-click-open", ["DOCUMENT"])



def wait_for(predicate, description):
    deadline = time.monotonic() + 20
    while not predicate():
        if time.monotonic() > deadline:
            raise RuntimeError(f"Timed out: {description}")
        time.sleep(0.05)


def phase2(session, root):
    working = root / "Operations"
    working.mkdir()
    (working / "alpha").write_text("alpha data")
    (working / "beta").write_text("beta data")
    session.address(working)
    session.observe("22-operation-list", ["alpha", "beta", "2 items"])
    # Rich preview rows need the browser space occupied by the open Shelf.
    session.click(848, 202)
    session.click(140, 160)
    session.observe("23-create-dialog", ["Name:", "New folder"])
    private.run_checked(["wtype", "Organized"], env=session.env)
    session.key("Return")
    wait_for(lambda: (working / "Organized").is_dir(), "create folder")
    session.observe("24-created-folder", ["Organized", "3 items"])
    session.click(242, 160)
    session.observe("25-create-file-dialog", ["New empty file", "Name:"])
    private.run_checked(["wtype", "note"], env=session.env)
    session.key("Return")
    wait_for(lambda: (working / "note").is_file(), "create empty file")
    assert (working / "note").read_bytes() == b""
    session.observe("26-created-file", ["note", "4 items"])
    session.key("F2")
    session.observe("27-rename-dialog", ["Rename", "Name:"])
    private.run_checked(["wtype", "renamed"], env=session.env)
    session.key("Return")
    wait_for(lambda: (working / "renamed").exists() and not (working / "note").exists(), "rename")
    session.observe("28-renamed", ["renamed", "4 items"])
    x, y = session.word("28-renamed", "alpha")
    session.click(x + 10, y)
    session.key("Down", "shift")
    session.observe("29-range", ["2 selected"])
    session.key("c", "ctrl")
    time.sleep(0.4)
    (session.output / "clipboard-types.txt").write_text(private.run_checked(["wl-paste", "--list-types"], env=session.env))
    session.address(working / "Organized")
    session.observe("30-paste-destination", ["This folder is empty"])
    (session.output / "clipboard-types-after-navigation.txt").write_text(private.run_checked(["wl-paste", "--list-types"], env=session.env))
    session.key("v", "ctrl")
    wait_for(lambda: (working / "Organized/alpha").exists() and (working / "Organized/beta").exists(), "copy selection")
    assert (working / "Organized/alpha").read_text() == "alpha data"
    assert (working / "alpha").read_text() == "alpha data"
    session.observe("31-copied", ["alpha", "beta", "2 items"])
    session.key("v", "ctrl")
    session.observe("32-conflict", ["Destination already exists", "Skip", "Replace"])
    x, y = session.word("32-conflict", "Skip", minimum_y=250)
    session.click(x - 35, y + 42)
    session.observe("33-second-conflict", ["Destination already exists", str(working / "beta")])
    x, y = session.word("33-second-conflict", "Skip", minimum_y=250)
    session.click(x - 35, y + 42)
    session.observe("34-skipped", ["2 skipped", "alpha", "beta"])
    session.address(working)
    session.observe("35-cut-source", ["renamed"])
    x, y = session.word("35-cut-source", "renamed")
    session.click(x + 10, y)
    session.key("x", "ctrl")
    session.address(working / "Organized")
    session.observe("36-cut-destination", ["alpha", "beta"])
    session.key("v", "ctrl")
    wait_for(lambda: (working / "Organized/renamed").exists() and not (working / "renamed").exists(), "cut and paste")
    time.sleep(0.5)
    session.observe("37-moved", ["renamed", "3 items"])
    x, y = session.word("37-moved", "renamed")
    session.click(x + 10, y)
    session.key("Delete")
    session.observe("38-trash-confirmation", ["Move to Trash?", "1 selected", "Enter confirms"])
    session.key("Return")
    wait_for(lambda: not (working / "Organized/renamed").exists(), "trash")
    session.observe("39-trashed", ["2 items"])
    session.click(630, 202)
    session.observe("40-trash-browser", ["Restore returns files", "renamed"])
    # Mounted-volume trash can contain unrelated records: select only our full path.
    x, y = session.trash_row("40-trash-browser", working / "Organized/renamed")
    session.click(x, y)
    session.observe("41-trash-selected", ["Restore selected"])
    x, y = session.word("41-trash-selected", "Restore", tree=True, minimum_y=200)
    session.click(x + 150, y - 34)
    wait_for(lambda: (working / "Organized/renamed").exists(), "restore trash entry")
    session.observe("42-restored", ["renamed", "3 items"])
    x, y = session.word("42-restored", "renamed")
    session.click(x + 10, y)
    session.key("Delete", "shift")
    session.observe("43-permanent-warning", ["Delete permanently?", "There is no Undo"])
    session.key("Escape")
    assert (working / "Organized/renamed").exists()
    session.observe("44-cancel-delete", ["3 items"])
    session.key("Delete", "shift")
    session.observe("44a-delete", ["Delete permanently?", "There is no Undo"])
    session.key("Return")
    wait_for(lambda: not (working / "Organized/renamed").exists(), "permanent deletion")
    session.observe("44b-deleted", ["2 items"])
    (working / "Organized/external").write_text("external change")
    session.observe("45-watch", ["external", "3 items"])
    session.key("a", "ctrl")
    session.observe("46-select-all", ["3 selected"])
    session.key("Escape")
    session.observe("47-clear-selection", ["0 selected"])
    x, y = session.word("47-clear-selection", "alpha")
    private.run_checked([str(session.pointer), "right-click", "1280", "800", str(x + 10), str(y)], env=session.env)
    session.observe("48-context-menu", ["1 selected", "New folder", "Copy"])
    x, y = session.word("48-context-menu", "Copy", minimum_y=300, last=True)
    session.click(x + 10, y)
    session.observe("49-context-copy", ["1 files held for copying"])
    time.sleep(0.3)
    payload = private.run_checked(["wl-paste", "--no-newline", "--type", "text/uri-list"], env=session.env)
    assert payload.strip() == (working / "Organized/alpha").as_uri()
    (session.output / "context-clipboard.txt").write_text(payload)


def isolated_runner(output, appearance, children):
    args = private.create_parser().parse_args(["--output-dir", str(output), "--appearance", appearance, "--allow-server-decorations"])
    config = private.prepare_configuration(args, output)
    with config["niri_config"].open("a") as file:
        file.write('window-rule { match app-id="little-clip-file-manager"; open-floating true; }\n')
    _, env, _ = private.start_compositors(config, args, output, children)
    env |= {"HOME": str(output / "home"), "XDG_DATA_HOME": str(output / "data")}
    for key in ["HOME", "XDG_DATA_HOME"]:
        Path(env[key]).mkdir()
    private.verify_private_wayland_target(env, config["runtime"])
    private.APP_ID = "little-clip-file-manager"
    return config, env


def finish_run(output, children, report):
    children.cleanup()
    (output / "report.json").write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps(report))
    return 0 if report["status"] == "passed" else 1


def application_arguments(description, binary, configure=None):
    parser = argparse.ArgumentParser(description=description)
    parser.add_argument("--output-dir", type=Path, required=True)
    parser.add_argument("--appearance", choices=["light", "dark"], required=True)
    parser.add_argument("--binary", type=Path, default=binary)
    if configure:
        configure(parser)
    return parser.parse_args()


def media_fixture_directory(args):
    output = args.output_dir.resolve()
    output.mkdir(mode=0o700, parents=True)
    root = output / "fixture"
    root.mkdir()
    return output, root


def run_application_acceptance(args, output, root, exercise, scope, prepare_environment=None):
    """Run a Files scenario with private compositor ownership and evidence capture."""
    children = private.ChildProcesses()
    report = {"status": "running", "appearance": args.appearance, "scope": scope}
    try:
        config, env = isolated_runner(output, args.appearance, children)
        if prepare_environment:
            prepare_environment(env, children)
        process = children.start("files", [str(args.binary.resolve()), str(root)], env=env,
                                 cwd=config["scratch"], log_path=output / "files.log")
        _, mapped_ms = private.wait_for_app_window(config["niri_bin"], env, process, output)
        session = Session(output, env, config["niri_bin"], process)
        exercise(session, root)
        session.key("q", "ctrl")
        process.wait(timeout=5)
        assert process.returncode == 0
        for pid in getattr(session, "required_closed_processes", []):
            assert not Path(f"/proc/{pid}").exists(), f"Files left playback process {pid} alive after closing"
        report.update(status="passed", mapped_ms=mapped_ms, observations=session.observations,
                      private_runtime=str(config["runtime"]))
    except Exception as error:
        report.update(status="failed", error=str(error))
        if "session" in locals():
            try:
                session.observe("failure-state", [])
            except Exception as capture_error:
                report["capture_error"] = str(capture_error)
            report["observations"] = session.observations
    finally:
        result = finish_run(output, children, report)
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output-dir", type=Path, required=True)
    parser.add_argument("--appearance", choices=["light", "dark"], required=True)
    parser.add_argument("--phase2-only", action="store_true")
    parser.add_argument("--binary", type=Path, default=Path(__file__).resolve().parent.parent / "target/debug/clip-file-manager")
    args = parser.parse_args()
    output = args.output_dir.resolve()
    output.mkdir(parents=True, mode=0o700)
    report = {"status": "running", "appearance": args.appearance}
    children = private.ChildProcesses()
    try:
        root = fixtures(output)
        config, env = isolated_runner(output, args.appearance, children)
        process = children.start("files", [str(args.binary.resolve()), str(root)],
            env=env | {"WAYLAND_DEBUG": "client"}, cwd=config["scratch"], log_path=output / "files.log")
        windows, mapped_ms = private.wait_for_app_window(config["niri_bin"], env, process, output)
        session = Session(output, env, config["niri_bin"], process)
        if not args.phase2_only:
            exercise(session, root)
        phase2(session, root)
        report.update({"observations": session.observations, "mapped_ms": mapped_ms, "windows": windows,
            "size_hints": private.wayland_size_hints(output / "files.log"),
            "private_runtime": str(config["runtime"]), "daemon_socket_exists": (config["runtime"] / "little-screen-clip.sock").exists()})
        session.key("q", "ctrl")
        process.wait(timeout=5)
        if process.returncode != 0:
            raise RuntimeError(f"file manager exited with {process.returncode}")
        report["status"] = "passed"
    except Exception as error:
        if "session" in locals():
            session.observe("failure-state", [])
        report.update({"status": "failed", "error": str(error)})
    finally:
        result = finish_run(output, children, report)
    return result


if __name__ == "__main__":
    raise SystemExit(main())
