#!/usr/bin/env python3
"""Validate native vectors and egui on one private OpenGL context."""
import json
from pathlib import Path
import re

from file_manager_acceptance import (
    application_arguments,
    media_fixture_directory,
    private,
    run_application_acceptance,
)


def exercise(session, _root):
    session.observe("01-shared-context", ["Vector context probe", "Zoom 100%", "Overlay clicks 0"])
    x, y = session.word("01-shared-context", "Magnify", tree=True, required=False) or session.word("01-shared-context", "Magnify")
    for _ in range(4):
        session.click(x + 12, y)
    session.observe("02-vector-zoom", ["Zoom 1600%", "Overlay clicks 0"])
    x, y = session.word("02-vector-zoom", "Overlay", tree=True)
    session.click(x + 20, y)
    session.click(x + 20, y)
    session.observe("03-egui-overlay", ["Overlay clicks 2", "Zoom 1600%"])
    x, y = session.word("03-egui-overlay", "Reset", tree=True, required=False) or session.word("03-egui-overlay", "Reset")
    session.click(x + 12, y)
    private.run_checked([str(session.pointer), "drag", "1280", "800", "700", "450", "760", "500"], env=session.env)
    for action in [["set-window-width", "850"], ["set-window-height", "600"]]:
        private.run_checked([str(session.niri), "msg", "action", *action], env=session.env)
    session.observe("04-resized", ["Zoom 100%", "Overlay clicks 2"])
    for action in [["set-window-width", "1100"], ["set-window-height", "720"]]:
        private.run_checked([str(session.niri), "msg", "action", *action], env=session.env)
    session.observe("05-restored", ["Zoom 100%", "Overlay clicks 2"])
    session.close()
    log = (session.output / "files.log").read_text()
    match = re.search(r"VECTOR_PROBE frames=(\d+) gl_errors=(\d+) layer_failures=(\d+) released=(\w+)", log)
    assert match, "probe must report native rendering and current-context cleanup"
    frames, errors, layers, released = match.groups()
    assert int(frames) >= 8, "exercise must paint repeated native vector frames"
    assert int(errors) == 0 and int(layers) == 0 and released == "true", match.group()
    (session.output / "vector-context.json").write_text(json.dumps({
        "frames": int(frames), "gl_errors": int(errors), "layer_failures": int(layers),
        "released": released == "true", "zoom_percent": 1600,
        "overlay_clicks": 2, "resized": True,
    }, indent=2))


def main():
    args = application_arguments(__doc__, Path("target/debug/examples/vector_context_probe"))
    output, root = media_fixture_directory(args)
    def environment(env, _children):
        private.APP_ID = "little-clip-vector-probe"
        env["LITTLE_CLIP_VECTOR_PROBE_APPEARANCE"] = args.appearance
        # Resize crossfades mix old/new native surfaces in screenshots. Disable
        # only this disposable compositor's animations so visual evidence shows
        # the actual frame produced by the two renderers.
        config = output / "niri.kdl"
        config.write_text(config.read_text() + "\nanimations { off; }\n")
        private.run_checked(["niri", "msg", "action", "load-config-file"], env=env)
    return run_application_acceptance(args, output, root, exercise,
                                      "shared egui/vector OpenGL context", environment)


if __name__ == "__main__":
    raise SystemExit(main())
