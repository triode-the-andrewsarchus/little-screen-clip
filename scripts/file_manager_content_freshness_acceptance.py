#!/usr/bin/env python3
"""Literal search freshness and multi-batch verification in the private compositor."""
from contextlib import contextmanager
import ctypes
import json
import os
from pathlib import Path
import select
import struct
import time

from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance
from file_manager_search_acceptance import submit


@contextmanager
def closed_files(directory):
    # Observe only owned fixture reads. No host input devices or compositor IPC.
    libc = ctypes.CDLL(None, use_errno=True)
    libc.inotify_init1.argtypes = [ctypes.c_int]
    libc.inotify_add_watch.argtypes = [ctypes.c_int, ctypes.c_char_p, ctypes.c_uint32]
    fd = libc.inotify_init1(os.O_NONBLOCK | os.O_CLOEXEC)
    if fd < 0:
        raise OSError(ctypes.get_errno(), "inotify_init1")
    try:
        if libc.inotify_add_watch(fd, os.fsencode(directory), 0x10) < 0:  # IN_CLOSE_NOWRITE
            raise OSError(ctypes.get_errno(), "inotify_add_watch")
        yield fd
    finally:
        os.close(fd)


def wait_for_successor(fd, process):
    deadline = time.monotonic() + 120
    saw_target = False
    while time.monotonic() < deadline:
        if process.poll() is not None:
            raise RuntimeError("Files exited before the controlled source edit")
        if not select.select([fd], [], [], 0.5)[0]:
            continue
        payload = os.read(fd, 65536)
        offset = 0
        while offset < len(payload):
            _, mask, _, length = struct.unpack_from("iIII", payload, offset)
            name = os.fsdecode(payload[offset + 16:offset + 16 + length].rstrip(b"\0"))
            offset += 16 + length
            if mask & 0x4000:  # IN_Q_OVERFLOW cannot establish the read sequence.
                raise RuntimeError("Fixture read observation overflowed")
            if name == "0500.txt":
                saw_target = True
            if saw_target and name == "0501.txt":
                # The next file is being read: the sequential search already
                # finished extracting and accepting the target's old match.
                return {"target": "0500.txt", "later_read": name}
    raise RuntimeError("Did not observe target and successor reads in order")


def exercise(session, root, expect_stale):
    session.observe("initial", ["Nested"])
    data = Path(session.env["XDG_DATA_HOME"]) / "little-screen-clip/search"
    assert not data.joinpath("index.sqlite").exists()
    target = root / "Nested/0500.txt"
    with closed_files(target.parent) as fd:
        submit(session, ':content:"violet lighthouse";scope:recursive')
        observed = wait_for_successor(fd, session.process)
        target.write_text("The replacement no longer contains the previous phrase.")
        expected = 1002 if expect_stale else 1001
        required = [f"{expected} matching entries", "Explicit search scope"]
        if not expect_stale:
            required.append("1 stale/unavailable omitted")
        session.observe("completed-after-edit", required, timeout_seconds=180)
    assert target.read_text() == "The replacement no longer contains the previous phrase."
    assert not data.joinpath("index.sqlite").exists()
    assert not data.joinpath("models").exists()
    assert not data.joinpath("runtime").exists()
    observed.update(expected_matches=expected, expected_stale_diagnostic=expect_stale,
                    fixtures=1002, no_implicit_index=True)
    (session.output / "source-edit-checks.json").write_text(json.dumps(observed, indent=2) + "\n")


def main():
    repository = Path(__file__).resolve().parent.parent
    def configure(parser):
        parser.add_argument("--expect-stale", action="store_true",
                            help="Diagnostic only: reproduce the old executable's stale result")
    args = application_arguments(__doc__, repository / "target/debug/clip-file-manager", configure)
    output, root = media_fixture_directory(args)
    nested = root / "Nested"
    nested.mkdir()
    for number in range(1002):
        (nested / f"{number:04}.txt").write_text("The violet lighthouse stands on a rocky coast.")
    return run_application_acceptance(args, output, root,
        lambda session, folder: exercise(session, folder, args.expect_stale),
        "Controlled edit after an offscreen file and its successor were read; final literal source verification across more than 1000 matches; no index/model/runtime")


if __name__ == "__main__":
    raise SystemExit(main())
