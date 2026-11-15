#!/usr/bin/env python3
"""Create a fresh, isolated, editable Phase 8 review desk and real project files."""
import argparse
import json
from pathlib import Path
import shlex
import uuid


def create(root, *, state_home=None):
    root = root.resolve()
    root.mkdir(parents=True, exist_ok=False)
    state = (state_home or root / ".state") / "little-screen-clip"
    state.mkdir(parents=True, exist_ok=True)
    for name in ["home.json", "projects.json", "organization.json"]:
        if (state / name).exists():
            raise FileExistsError(f"Refusing to replace {state / name}")
    projects = []
    for folder, title, tasks in [
        ("field-recorder", "Field recorder", ["Pack spare batteries", "Record morning birds", "Tag the best takes"]),
        ("garden", "Courtyard garden", ["Sketch the planting beds", "Order herb seeds", "Check the watering plan"]),
    ]:
        path = root / "projects" / folder
        path.mkdir(parents=True)
        identifier = str(uuid.uuid4())
        (path / ".fmproject").write_text(json.dumps({"version": 1, "id": identifier,
            "title": title, "description": "A real folder outside the Home layout.",
            "readme": "README.md", "todo": "TODO.md"}, indent=2) + "\n")
        (path / "README.md").write_text(f"# {title}\n\nProject files live here. Home only keeps references.\n")
        (path / "TODO.md").write_text("# Next steps\n\n" + "".join(f"- [ ] {task}\n" for task in tasks))
        projects.append({"id": identifier, "title": title, "folder": str(path)})
    notes = root / "notes"
    notes.mkdir()
    (notes / "ideas.md").write_text("# Small ideas\n\nTry a new walking route. Keep the interesting sounds.\n")
    (notes / ".hide").write_text('version = 1\npatterns = ["*.tmp"]\n')
    (notes / ".show").write_text('version = 1\npatterns = [".field-notes.md"]\n')
    (notes / ".field-notes.md").write_text("# Field notes\n\nDeliberately revealed by .show.\n")
    (notes / "scratch.tmp").write_text("Deliberately hidden; Rules off reveals this.\n")
    pins = [{"id": str(uuid.uuid4()), "path": str(path), "folder": folder}
            for path, folder in [(notes, True), (notes / "ideas.md", False)]]
    elements = []
    bindings = []

    def ink(identifier, x, y, points, color, pin=None):
        elements.append({"id": identifier, "type": "freedraw", "x": x, "y": y,
            "width": max(p[0] for p in points), "height": max(p[1] for p in points),
            "points": points, "strokeColor": color, "strokeWidth": 3,
            "backgroundColor": "transparent", "fillStyle": "solid", "opacity": 100,
            "roughness": 1, "seed": 42, "angle": 0})
        if pin:
            bindings.append({"element": identifier, "pin": pin})

    def text(identifier, x, y, value, size=26, color="#161616", pin=None):
        elements.append({"id": identifier, "type": "text", "x": x, "y": y,
            "width": len(value) * size * .58, "height": size * 1.4, "text": value,
            "fontSize": size, "fontFamily": 1, "strokeColor": color,
            "backgroundColor": "transparent", "opacity": 100, "angle": 0})
        if pin:
            bindings.append({"element": identifier, "pin": pin})

    text("welcome", 45, 15, "A place for the things in progress", 30)
    text("notes-label", 80, 210, "Field notes", 28, pin=pins[0]["id"])
    ink("notebook", 95, 100, [[0, 8], [85, 0], [90, 91], [2, 98], [0, 8],
        [15, 7], [17, 97], [19, 97], [17, 7]], "#161616", pins[0]["id"])
    ink("notebook-lines", 128, 124, [[0, 0], [37, 0], [0, 0], [0, 19], [37, 19]], "#161616", pins[0]["id"])
    text("ideas-label", 305, 210, "Small ideas", 28, pin=pins[1]["id"])
    ink("idea-star", 330, 110, [[35, 0], [47, 25], [75, 28], [54, 47], [60, 77],
        [35, 62], [8, 77], [15, 47], [0, 28], [26, 25], [35, 0]], "#161616", pins[1]["id"])
    text("annotation", 80, 295, "Click the ink to go there", 20)
    ink("underline", 72, 260, [[0, 4], [120, 0], [250, 3], [410, 0]], "#161616")
    text("reminder", 85, 420, "Leave room for the next idea...", 23)
    text("source-note", 640, 590, "These checkboxes edit the real TODO files", 19)
    cards = [{"id": str(uuid.uuid4()), "rect": [620, 80 + index * 255, 570, 240],
              "content": {"kind": "project", "project": project["id"], "title": "Project tasks"}}
             for index, project in enumerate(projects)]
    home = {"version": 2, "pins": pins, "board": {"revision": 0,
        "title": "My little field desk", "drawing": json.dumps({"type": "excalidraw",
        "version": 2, "elements": elements, "appState": {"viewBackgroundColor": "#ffffff"}, "files": {}}),
        "cards": cards, "bindings": bindings, "view": None}}
    for name, content in [("home.json", home), ("projects.json", {"version": 1, "projects": projects})]:
        with (state / name).open("x") as output:
            json.dump(content, output, indent=2)
            output.write("\n")
    if state_home is None:
        (root / ".home").mkdir()
        (root / ".config").mkdir()
        binary = Path(__file__).resolve().parents[1] / "target/debug/clip-file-manager"
        launch = root / "open-review.sh"
        launch.write_text("#!/bin/sh\nset -eu\n" + "\n".join(
            f"export {key}={shlex.quote(str(value))}" for key, value in [
                ("HOME", root / ".home"), ("XDG_STATE_HOME", root / ".state"),
                ("XDG_CONFIG_HOME", root / ".config")]) + f"\ncd {shlex.quote(str(root))}\nexec {shlex.quote(str(binary))}\n")
        launch.chmod(0o755)
        (root / "README.md").write_text("# Phase 8 review desk\n\nRun `./open-review.sh` to open an isolated Home. "
            "Your normal Home, bookmarks and configuration are untouched.\n\n"
            "Click the notebook, star or their labels. Check a project task, then inspect "
            "its `projects/*/TODO.md`: only that real source changes. Unlock objects to place and resize content, "
            "move each project panel or rename the desk. Pan/zoom, quit and reopen.\n\n"
            "In notes, Rules off reveals scratch.tmp; Rules on deliberately reveals .field-notes.md. "
            "Session marks and Sets are independent controls in the browser.\n\n"
            "The generator refuses an existing destination so rerunning cannot erase your changes.\n")
    return home


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("directory", type=Path)
    args = parser.parse_args()
    create(args.directory)
    print(args.directory.resolve() / "open-review.sh")
