#!/usr/bin/env python3
"""Exercise the standalone file manager on the existing private Weston/Niri runner.

All keyboard/pointer input and screenshots target this run's private sockets.
Uses synthetic files and leaves the user's compositor, daemon, and clipboard alone.
"""

import argparse
import csv
import io
import json
import time
from pathlib import Path

from PIL import Image

import ui_acceptance_capture as private


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

    def observe(self, name, expected):
        # Poll rendered output, not a guessed fixed delay, until the requested
        # visible evidence appears. Keep the PNG and extracted text for review.
        deadline = time.monotonic() + 10
        path = self.output / f"{name}.png"
        while True:
            if self.process.poll() is not None:
                raise RuntimeError("file manager exited during acceptance")
            private.screenshot(self.niri, self.env, path)
            ocr_path = self.output / f"{name}-ocr.png"
            with Image.open(path) as screen:
                screen.resize((screen.width * 3, screen.height * 3)).save(ocr_path)
            text = private.run_checked(["tesseract", str(ocr_path), "stdout", "--psm", "11"], env=self.env)
            visible = "".join(text.lower().split())
            if all("".join(fragment.lower().split()) in visible for fragment in expected):
                break
            if time.monotonic() > deadline:
                raise RuntimeError(f"{name}: missing {expected!r} in rendered text: {text!r}")
            time.sleep(0.15)
        (self.output / f"{name}.txt").write_text(text)
        self.observations.append({"name": name, "screenshot": str(path), "expected_visible_text": expected})

    def word(self, name, text, tree=False, required=True):
        data = private.run_checked([
            "tesseract", str(self.output / f"{name}-ocr.png"), "stdout", "--psm", "11", "-c", "tessedit_create_tsv=1",
        ], env=self.env)
        for word in csv.DictReader(io.StringIO(data), delimiter="\t"):
            if word["text"].lower() != text.lower():
                continue
            x, y = int(word["left"]) // 3, int(word["top"]) // 3
            if (x < 330) == tree:
                return x, y + int(word["height"]) // 6
        if required:
            raise RuntimeError(f"{name}: no {text!r} in {'tree' if tree else 'listing'}")
        return None


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


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output-dir", type=Path, required=True)
    parser.add_argument("--appearance", choices=["light", "dark"], required=True)
    parser.add_argument("--binary", type=Path, default=Path(__file__).resolve().parent.parent / "target/debug/clip-file-manager")
    args = parser.parse_args()
    output = args.output_dir.resolve()
    output.mkdir(parents=True, mode=0o700)
    report = {"status": "running", "appearance": args.appearance}
    children = private.ChildProcesses()
    try:
        root = fixtures(output)
        runner_args = private.create_parser().parse_args([
            "--output-dir", str(output), "--appearance", args.appearance, "--allow-server-decorations",
        ])
        config = private.prepare_configuration(runner_args, output)
        with config["niri_config"].open("a") as file:
            file.write('window-rule { match app-id="little-clip-file-manager"; open-floating true; }\n')
        _, env, _ = private.start_compositors(config, runner_args, output, children)
        private.verify_private_wayland_target(env, config["runtime"])
        private.APP_ID = "little-clip-file-manager"
        process = children.start("files", [str(args.binary.resolve()), str(root)],
            env=env | {"WAYLAND_DEBUG": "client"}, cwd=config["scratch"], log_path=output / "files.log")
        windows, mapped_ms = private.wait_for_app_window(config["niri_bin"], env, process, output)
        session = Session(output, env, config["niri_bin"], process)
        exercise(session, root)
        report.update({"observations": session.observations, "mapped_ms": mapped_ms, "windows": windows,
            "size_hints": private.wayland_size_hints(output / "files.log"),
            "private_runtime": str(config["runtime"]), "daemon_socket_exists": (config["runtime"] / "little-screen-clip.sock").exists()})
        session.key("q", "ctrl")
        process.wait(timeout=5)
        if process.returncode != 0:
            raise RuntimeError(f"file manager exited with {process.returncode}")
        report["status"] = "passed"
    except Exception as error:
        report.update({"status": "failed", "error": str(error)})
    finally:
        children.cleanup()
        (output / "report.json").write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps(report))
    return 0 if report["status"] == "passed" else 1


if __name__ == "__main__":
    raise SystemExit(main())
