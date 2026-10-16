#!/usr/bin/env python3
"""Verify inspector URI dragging between clients on private Weston/Niri."""
import json
import shutil
from pathlib import Path
import ui_acceptance_capture as private
from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance


def main():
    args = application_arguments(__doc__, Path("target/release/clip-file-manager"))
    output, root = media_fixture_directory(args)
    (root / "a-network").symlink_to("/sys/class/net/lo", target_is_directory=True)
    destination = output / "RECEIVER"
    destination.mkdir()
    receiver = None

    def prepare(env, children):
        nonlocal receiver
        env["WAYLAND_DEBUG"] = "client"
        receiver = children.start("receiver", [str(args.binary.resolve()), str(destination)],
                                  env=env, cwd=destination, log_path=output / "receiver.log")
        private.wait_for_app_window(Path(shutil.which("niri")), env, receiver, output)

    def exercise(session, _):
        session.observe("01-network", ["a-network", "Carrier Up"])
        x, y = session.word("01-network", "a-network")
        session.click(x + 32, y + 55)
        session.observe("02-inspector", ["Network interface", "Operational state"])
        private.wait_for_app_window(session.niri, session.env, receiver, output)
        windows = json.loads(private.run_checked(
            [str(session.niri), "msg", "--json", "windows"], env=session.env))
        for process, width, left in [(session.process, 640, 0), (receiver, 640, 640)]:
            window = next(window for window in windows if window["pid"] == process.pid)
            for action in [["set-window-width", "--id", str(window["id"]), str(width)],
                           ["set-window-height", "--id", str(window["id"]), "760"],
                           ["move-floating-window", "--id", str(window["id"]),
                            "--x", str(left), "--y", "0"]]:
                private.run_checked([str(session.niri), "msg", "action", *action], env=session.env)
        session.observe("02b-two-clients", ["Network interface", "RECEIVER", "Shelf"])
        session.click(855, 219)  # Close the receiver Shelf to expose operation results.
        session.scroll_to_visible("03-relationships", ["Related kernel objects", "Browse folder"],
                                  step=150, attempts=24, position=(500, 600))
        x, y = session.word("03-relationships", "Kernel", minimum_y=300, last=True, tree=True)
        private.run_checked([str(session.pointer), "drag", "1280", "800",
                             str(x + 12), str(y), "1150", "450"], env=session.env)
        session.observe("04-reference-received", ["Network interface", "0 completed", "1 failed"])
        # Receipt must reach the ordinary mutation safety admission, which refuses
        # kernel payload copying. Offering a MIME type alone is insufficient proof.
        # Failed reports are already expanded. Widen the receiver so the full
        # reason remains readable alongside its long captured kernel path.
        for action in [["set-window-width", "--id", str(window["id"]), "1100"],
                       ["move-floating-window", "--id", str(window["id"]), "--x", "90", "--y", "0"]]:
            private.run_checked([str(session.niri), "msg", "action", *action], env=session.env)
        session.observe("05-admission-refused", ["Safety admission", "inspection-only", "0 completed", "1 failed"])
        assert list(destination.iterdir()) == [], "reference drop must not copy kernel payloads"
        source_log = (output / "files.log").read_text()
        receiver_log = (output / "receiver.log").read_text()
        assert 'offer("text/uri-list")' in source_log
        assert 'set_actions(1)' in source_log, "source must advertise Copy only"
        assert 'receive("text/uri-list"' in receiver_log
        (output / "native-reference.json").write_text(json.dumps({
            "uri_received": True, "copy_only": True, "mutation_refused": True,
            "destination_unchanged": True,
        }, indent=2))
        session.click(1000, 47)
        session.key("q", "ctrl")
        receiver.wait(timeout=5)
        assert receiver.returncode == 0
        session.click(200, 47)

    return run_application_acceptance(args, output, root, exercise,
                                      "inspector native reference delivery", prepare)


if __name__ == "__main__":
    raise SystemExit(main())
