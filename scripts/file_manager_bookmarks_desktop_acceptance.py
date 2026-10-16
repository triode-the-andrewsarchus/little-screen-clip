#!/usr/bin/env python3
"""Cross-client bookmark drops must preserve originals; ordinary moves still work."""
import json
from pathlib import Path

import ui_acceptance_capture as private
from file_manager_acceptance import Session, application_arguments, finish_run, wait_for
from file_manager_desktop_acceptance import drag, position, start_two_clients


def main():
    args = application_arguments(__doc__, Path(__file__).resolve().parent.parent / "target/debug/clip-file-manager")
    output = args.output_dir.resolve()
    output.mkdir(mode=0o700, parents=True)
    left, right = output / "LEFT", output / "RIGHT"
    left.mkdir()
    right.mkdir()
    for name in ["BOOK", "MOVE", "COPY"]:
        (left / name).mkdir()
        (left / name / "payload").write_text(name)
    unreadable = left / "BOOK/unreadable"
    unreadable.mkdir()
    unreadable.chmod(0)
    children = private.ChildProcesses()
    report = {"status": "running", "appearance": args.appearance}
    try:
        config, env, apps = start_two_clients(output, args.appearance, args.binary, [left, right], children)
        session = Session(output, env, config["niri_bin"], apps[0])
        session.observe("two-clients", ["BOOK", "MOVE", "COPY"])
        drag(session, position(session, "two-clients", "BOOK", True), (780, 462))
        shared = output / "config/gtk-3.0/bookmarks"
        wait_for(lambda: shared.exists() and (left / "BOOK").as_uri() in shared.read_text(), "cross-client reference-only bookmark")
        assert (left / "BOOK/payload").read_text() == "BOOK"
        assert not (right / "BOOK").exists()
        session.observe("shared-bookmarks", ["Bookmarks", "BOOK", "MOVE"])
        drag(session, position(session, "shared-bookmarks", "MOVE", True), (1090, 390))
        wait_for(lambda: (right / "MOVE/payload").exists() and not (left / "MOVE").exists(), "ordinary native directory move")
        assert (right / "MOVE/payload").read_text() == "MOVE"
        session.observe("moved", ["MOVE", "COPY"])
        drag(session, position(session, "moved", "COPY", True), (1090, 390), copy=True)
        wait_for(lambda: (right / "COPY/payload").exists(), "ordinary native directory copy")
        assert (left / "COPY/payload").read_text() == (right / "COPY/payload").read_text() == "COPY"
        session.observe("copied", ["Bookmarks", "BOOK", "COPY", "MOVE"])
        report.update(status="passed", observations=session.observations, private_runtime=str(config["runtime"]))
    except Exception as error:
        report.update(status="failed", error=str(error))
        if "session" in locals():
            session.observe("failure-state", [])
    finally:
        unreadable.chmod(0o700)
        result = finish_run(output, children, report)
    return result


if __name__ == "__main__":
    raise SystemExit(main())
