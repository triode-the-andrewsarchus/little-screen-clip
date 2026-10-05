#!/usr/bin/env python3
"""Test actual Wayland file drops between two clients on private Weston/Niri."""
import argparse
import csv
import io
import json
import time
import subprocess
from pathlib import Path
import ui_acceptance_capture as private
from file_manager_acceptance import Session, wait_for, isolated_runner, finish_run


def position(session, name, word, left):
    data = private.run_checked(["tesseract", str(session.output / f"{name}-ocr.png"), "stdout", "--psm", "11", "-c", "tessedit_create_tsv=1", "-c", "tessedit_create_txt=0"], env=session.env)
    for item in csv.DictReader(io.StringIO(data), delimiter="\t", quoting=csv.QUOTE_NONE):
        if item["text"].lower() != word.lower():
            continue
        x, y = int(item["left"]) // 3, int(item["top"]) // 3
        if (250 < x < 630 if left else 890 < x < 1270) and y > 230:
            return x + 12, y + int(item["height"]) // 6
    raise RuntimeError(f"{word}: no {'left' if left else 'right'} file row")


def drag(session, source, destination, copy=False):
    session.click(*source)
    if copy:
        modifier = subprocess.Popen(["wtype", "-M", "ctrl", "-s", "2500", "-m", "ctrl"], env=session.env)
        time.sleep(0.15)
    try:
        private.run_checked([str(session.pointer), "drag", "1280", "800", *map(str, (*source, *destination))], env=session.env)
    finally:
        if copy:
            modifier.wait(timeout=4)
    time.sleep(0.3)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output-dir", type=Path, required=True)
    parser.add_argument("--appearance", choices=["light", "dark"], required=True)
    args = parser.parse_args()
    output = args.output_dir.resolve()
    output.mkdir(mode=0o700)
    children = private.ChildProcesses()
    report = {"status": "running", "appearance": args.appearance}
    try:
        left, right = output / "LEFT", output / "RIGHT"
        left.mkdir(); right.mkdir()
        (left / "COPYME").write_text("copy payload")
        (left / "MOVEME").write_text("move payload")
        (right / "RETURNME").write_text("return payload")
        (left / "Nested").mkdir()
        (left / "TREE").mkdir(); (left / "TREE/data").write_text("tree payload")
        config, env = isolated_runner(output, args.appearance, children)
        binary = Path(__file__).resolve().parent.parent / "target/debug/clip-file-manager"
        apps = []
        for index, root in enumerate([left, right]):
            process = children.start(f"files-{index}", [str(binary), str(root)], env=env | {"WAYLAND_DEBUG": "client"}, cwd=config["scratch"], log_path=output / f"files-{index}.log")
            windows, _ = private.wait_for_app_window(config["niri_bin"], env, process, output)
            window_id = str(windows[0]["id"])
            for action in [["set-window-width", "--id", window_id, "640"], ["set-window-height", "--id", window_id, "700"], ["move-floating-window", "--id", window_id, "--x", str(index * 640), "--y", "0"]]:
                private.run_checked([str(config["niri_bin"]), "msg", "action", *action], env=env)
            apps.append(process)
        session = Session(output, env, config["niri_bin"], apps[0])
        session.observe("01-two-clients", ["COPYME", "MOVEME", "RETURNME"])
        drag(session, position(session, "01-two-clients", "COPYME", True), (1090, 470), copy=True)
        wait_for(lambda: (right / "COPYME").exists(), "native desktop copy")
        assert (right / "COPYME").read_text() == (left / "COPYME").read_text() == "copy payload"
        session.observe("02-copied", ["COPYME", "RETURNME"])
        drag(session, position(session, "02-copied", "MOVEME", True), (1090, 470))
        wait_for(lambda: (right / "MOVEME").exists() and not (left / "MOVEME").exists(), "native desktop move")
        assert (right / "MOVEME").read_text() == "move payload"
        session.observe("03-moved", ["MOVEME", "RETURNME"])
        drag(session, position(session, "03-moved", "RETURNME", False), (470, 470))
        wait_for(lambda: (left / "RETURNME").exists() and not (right / "RETURNME").exists(), "reverse native move")
        assert (left / "RETURNME").read_text() == "return payload"
        session.observe("04-returned", ["RETURNME", "TREE"])
        drag(session, position(session, "04-returned", "TREE", True), (1090, 470), copy=True)
        wait_for(lambda: (right / "TREE/data").exists(), "native recursive copy")
        assert (right / "TREE/data").read_text() == (left / "TREE/data").read_text()
        session.observe("05-tree-copy", ["TREE", "COPYME"])
        drag(session, position(session, "05-tree-copy", "COPYME", True), (470, 770))
        time.sleep(0.5)
        assert (left / "COPYME").read_text() == "copy payload"
        session.observe("06-cancelled", ["COPYME", "RETURNME"])
        # A same-client drop still passes through the native protocol and targets a folder row.
        drag(session, position(session, "06-cancelled", "RETURNME", True), position(session, "06-cancelled", "Nested", True))
        wait_for(lambda: (left / "Nested/RETURNME").exists() and not (left / "RETURNME").exists(), "native same-window folder move")
        session.observe("07-internal-folder", ["Nested", "COPYME"])
        drag(session, position(session, "07-internal-folder", "COPYME", True), (1090, 470))
        session.observe("08-move-conflict", ["Destination already exists", "Skip"])
        x, y = session.word("08-move-conflict", "Skip", minimum_y=250)
        session.click(x - 35, y + 42)
        session.observe("09-skipped-move", ["1 skipped"])
        assert (left / "COPYME").read_text() == (right / "COPYME").read_text() == "copy payload"
        session.click(80, 315)
        session.observe("10-tree-target", ["Nested", "COPYME"])
        drag(session, position(session, "10-tree-target", "COPYME", True), session.word("10-tree-target", "Nested", tree=True))
        wait_for(lambda: (left / "Nested/COPYME").exists() and not (left / "COPYME").exists(), "native tree-folder move")
        session.observe("11-tree-moved", ["Nested", "TREE"])
        report.update(status="passed", observations=session.observations, private_runtime=str(config["runtime"]))
    except Exception as error:
        if "session" in locals():
            session.observe("failure-state", [])
        report.update(status="failed", error=str(error))
    finally:
        result = finish_run(output, children, report)
    return result

if __name__ == "__main__":
    raise SystemExit(main())
