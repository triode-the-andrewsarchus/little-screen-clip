#!/usr/bin/env python3
"""Exercise read-only system inspection on the private Weston/Niri desktop."""
from pathlib import Path
import subprocess
import time
import ui_acceptance_capture as private

from file_manager_acceptance import (
    application_arguments, media_fixture_directory, run_application_acceptance,
)


def fixtures(root):
    # References only: no device opens or kernel writes manufacture fixtures.
    (root / "a-network").symlink_to("/sys/class/net/lo", target_is_directory=True)
    (root / "b-null").symlink_to("/dev/null")
    (root / "c-kernel").symlink_to("/proc/uptime")
    (root / "d-notes.txt").write_text("Ordinary text remains readable.\n" * 20)


def exercise(session, root):
    session.observe("01-ready", ["a-network", "b-null", "Carrier Up"])
    session.click(848, 201)
    session.observe("01-mixed", ["a-network", "b-null", "d-notes.txt", "Carrier Up"])
    # Modifier clicks on system preview icons must select, not inspect/open.
    x, y = session.word("01-mixed", "a-network")
    session.click(x + 30, y)
    session.observe("01a-first-selected", ["1 selected", "a-network", "b-null"])
    for modifier, source, target, count in [
        ("ctrl", "01a-first-selected", "b-null", "2 selected"),
        ("shift", "01a-first-selected", "c-kernel", "3 selected"),
    ]:
        # Reset the range anchor so Shift spans the directory and both nodes.
        if modifier == "shift":
            x, y = session.word("01b-ctrl-selected", "a-network")
            session.click(x + 30, y)
        x, y = session.word(source, target)
        modifier_process = subprocess.Popen(
            ["wtype", "-M", modifier, "-s", "1200", "-m", modifier], env=session.env)
        try:
            time.sleep(.15)
            session.click(x + 32, y + 55)
        finally:
            modifier_process.wait(timeout=3)
        session.observe("01b-ctrl-selected" if modifier == "ctrl" else "01c-shift-selected",
                        [count, "a-network", "b-null", "c-kernel"])
    x, y = session.word("01-mixed", "a-network")
    session.click(x + 30, y, delay=.03)
    session.click(x + 30, y)
    session.observe("02-directory-navigation", ["/sys/devices/virtual/net/lo", "queues"])
    session.key("Left", "alt")
    session.observe("03-browser-back", ["a-network", "b-null"])
    x, y = session.word("03-browser-back", "a-network")
    session.click(x + 32, y + 55)
    session.observe("04-network-inspector", ["Network interface", "Namespace view", "Operational state"])
    # Address/queue inventories extend this inspector beyond its original fields.
    for _ in range(6):
        private.run_checked([str(session.pointer), "scroll", "1280", "800", "700", "600", "600"], env=session.env)
    session.observe("04b-relationships", ["Related kernel objects", "Browse folder"])
    # Drag the link itself without a preliminary click (which would navigate).
    x, y = session.word("04b-relationships", "Kernel", minimum_y=500, last=True, tree=True)
    private.run_checked([str(session.pointer), "drag", "1280", "800",
                         str(x + 12), str(y), "120", "70"], env=session.env)
    session.observe("04c-drag-cancel", ["Network interface", "Related kernel objects", "Browse folder"])
    protocol = (session.output / "files.log").read_text()
    assert 'offer("text/uri-list")' in protocol, "relationship must offer native file references"
    assert 'set_actions(1)' in protocol, "relationship drag must advertise Copy only"
    x, y = session.word("04c-drag-cancel", "folder", minimum_y=500)
    session.click(x + 15, y)
    session.observe("05-browse-relationship", ["/sys/devices/virtual/net/lo", "queues"])
    session.key("Left", "alt")
    session.observe("06-browser-history", ["a-network", "b-null"])
    x, y = session.word("06-browser-history", "b-null")
    session.click(x + 25, y)
    session.key("Return")
    session.observe("07-pseudo-device", ["Interface semantics", "Discard sink", "Unverified", "Back to folder"])
    session.key("Escape")
    session.observe("08-return", ["a-network", "b-null"])
    session.key("Down")
    session.key("Return")
    session.observe("09-unknown-kernel", ["Attribute policy", "Unknown attribute", "content not read"])
    session.key("Escape")
    session.observe("10-ordinary-return", ["d-notes.txt"])
    x, y = session.word("10-ordinary-return", "d-notes.txt")
    session.click(x + 25, y)
    session.key("Return")
    session.observe("11-ordinary-text", ["Ordinary text remains readable", "Edit"])
    session.key("Escape")
    session.observe("12-final-folder", ["a-network", "b-null"])


def main():
    args = application_arguments(__doc__, Path("target/release/clip-file-manager"))
    output, root = media_fixture_directory(args)
    fixtures(root)
    return run_application_acceptance(
        args, output, root, exercise, "system inspector navigation and reference dragging",
        prepare_environment=lambda env, _: env.update(WAYLAND_DEBUG="client"))


if __name__ == "__main__":
    raise SystemExit(main())
