#!/usr/bin/env python3
"""Exercise the native Home desk, persistent references and guarded handoffs."""
import json
from PIL import Image, ImageOps
from pathlib import Path
import ui_acceptance_capture as private
from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance, wait_for
from file_manager_phase4_acceptance import click_word as sparse_click


def click_word(session, shot, word, *, minimum_y=85, maximum_y=None, control_bounds=None):
    try:
        sparse_click(session, shot, word, minimum_y=minimum_y, maximum_y=maximum_y)
    except RuntimeError:
        # Reuse the runner's alternate OCR layout for plain, low-contrast
        # toolbar text. Keep the target inside the original requested band.
        for tree in [True, False]:
            point = session.word(shot, word, tree=tree, required=False, minimum_y=minimum_y)
            if point and (maximum_y is None or point[1] < maximum_y):
                session.click(point[0] + 5, point[1]); return
        if minimum_y < 145:
            name = shot + "-header"
            with Image.open(session.output / f"{shot}.png") as picture:
                isolated = Image.new("L", picture.size, 255)
                isolated.paste(ImageOps.autocontrast(picture.crop((96,96,900,140)).convert("L")), (96,96))
                isolated.resize((picture.width*3,picture.height*3)).save(session.output / f"{name}-ocr.png")
            for tree in [True, False]:
                point = session.word(name, word, tree=tree, required=False, minimum_y=minimum_y)
                if point and (maximum_y is None or point[1] < maximum_y):
                    session.click(point[0] + 5, point[1]); return
        controls = {"Arrange":(400,104,462,131), "Undo":(540,104,584,131), "Redo":(606,104,649,131), "Fit":(670,104,695,131), "Link":(105,148,230,174)}
        if control_bounds is not None or (minimum_y < 145 and word in controls):
            name = shot + "-control"
            bounds = control_bounds or controls[word]
            with Image.open(session.output / f"{shot}.png") as picture:
                isolated = Image.new("L", picture.size, 255)
                isolated.paste(ImageOps.autocontrast(picture.crop(bounds).convert("L")), bounds[:2])
                isolated.resize((picture.width*3,picture.height*3)).save(session.output / f"{name}-ocr.png")
            for tree in [True, False]:
                point = session.word(name, word, tree=tree, required=False, minimum_y=minimum_y)
                if point: session.click(point[0] + 2, point[1]); return
        raise


def enter_home(session, shot):
    # Captured browser toolbar; native Home contents are independently asserted.
    point = session.word(shot, "Home", required=False)
    session.click(*(point if point else (995, 118)))


def add(session, shot, word, name):
    click_word(session, shot, "Add", minimum_y=90, maximum_y=145)
    session.observe(name, ["Pin current folder", "Pin selection", "Named section"])
    click_word(session, name, word, minimum_y=135, maximum_y=340)


def drag(session, start, end):
    private.run_checked([str(session.pointer), "drag", *map(str, session.logical_size),
        *map(str, (*start, *end))], env=session.env)


def resize_handle(session, shot):
    with Image.open(session.output / f"{shot}.png").convert("RGB") as picture:
        pixels = picture.load()
        handle = [(x,y) for y in range(260,736) for x in range(320,1182)
                  if all(abs(value-target) <= 2 for value,target in zip(pixels[x,y], (64,126,150)))]
    assert len(handle) > 6, "the selected live card must paint its real resize handle"
    return max(x for x,y in handle)-5, max(y for x,y in handle)-5


def artwork_target(session, shot, element, view):
    # Find the actual white paper in the captured frame, then verify native ink
    # at the authored source point under the persisted camera transform. This
    # avoids targeting another similarly named live card or a source text field.
    session.capture_text(shot, session.output / f"{shot}.png", [])
    with Image.open(session.output / f"{shot}.png").convert("RGB") as picture:
        pixels = picture.load()
        rows = [y for y in range(140, 740) if sum(min(pixels[x,y]) > 253 for x in range(96,1184)) > 900]
        if not rows: return None
        top, bottom = min(rows), max(rows) + 1
        white = [x for x in range(96,1184) if min(pixels[x,top+8]) > 253]
        left, right = min(white), max(white) + 1
        scale = view["scale"]
        cx, cy = view["center"]
        point = element["points"][len(element["points"])//2]
        x = round((left+right)/2 + (element["x"]+point[0]-cx)*scale)
        y = round((top+bottom)/2 + (element["y"]+point[1]-cy)*scale)
        ink = [(px,py) for py in range(y-6,y+7) for px in range(x-6,x+7) if max(pixels[px,py]) < 120]
        return min(ink, key=lambda p: (p[0]-x)**2+(p[1]-y)**2) if ink else None


def exercise(session, root, binary):
    session.ocr_regions = [("contrast-line-home-header.png", (96,96,900,140)),
        ("contrast-line-home-undo.png", (540,104,584,131)),
        ("contrast-line-home-redo.png", (606,104,649,131)),
        ("contrast-line-home-fit.png", (670,104,695,131))]
    state = Path(session.env["XDG_STATE_HOME"]) / "little-screen-clip/home.json"
    def saved():
        return json.loads(state.read_text()) if state.exists() else {"pins": [], "board": {"cards": []}}
    def note_card():
        target = next((pin["id"] for pin in saved()["pins"] if pin["path"] == str(root / "note.txt")), None)
        return next((card for card in saved()["board"]["cards"] if card["content"].get("pin") == target), None)

    session.observe("folder", ["note.txt", "List"])
    session.click(600, 160)
    session.observe("selected-file", ["note.txt", "Name"])
    x, y = session.word("selected-file", "note.txt", minimum_y=240)
    session.click(x + 10, y)
    enter_home(session, "selected-file")
    session.observe("empty-desk", ["Home", "Arrange", "Make this place yours"])
    add(session, "empty-desk", "selection", "add-file")
    wait_for(lambda: len(saved()["pins"]) == 1, "selected file reference saved")
    session.observe("file-pin", ["note.txt", "Home"])
    add(session, "file-pin", "current", "add-folder")
    wait_for(lambda: len(saved()["pins"]) == 2, "folder reference saved")
    session.observe("two-pins", ["note.txt", "fixture", "Arrange"])
    ids = [pin["id"] for pin in saved()["pins"]]
    click_word(session, "two-pins", "Arrange", minimum_y=90, maximum_y=145)
    session.observe("arrange", ["note.txt", "Add", "Fit"])
    add(session, "arrange", "Named", "add-section")
    session.observe("new-section", ["A new corner", "Label"])
    # Resolve the inspector from its observed label, not the artwork below it.
    lx, ly = session.word("new-section", "Label", tree=True, minimum_y=130)
    # OCR sometimes merges the label with the separator above it. The captured
    # inspector field occupies y=148..172; use its interior rather than that box.
    session.click(lx + 90, max(156, ly))
    session.key("a", "ctrl")
    private.run_checked(["wtype", "Projects"], env=session.env)
    session.key("Return")
    wait_for(lambda: any(c["content"].get("title") == "Projects" for c in saved()["board"]["cards"]), "section title saved")
    session.observe("named-section", ["Projects", "note.txt"])
    before = note_card()["rect"][:]
    nx, ny = session.word("named-section", "note.txt", minimum_y=150)
    drag(session, (nx + 20, ny), (nx + 160, ny + 180))
    wait_for(lambda: note_card()["rect"][0] > before[0] + 30, "shortcut moved in world coordinates")
    moved = note_card()["rect"][:]
    session.observe("moved-shortcut", ["note.txt", "Undo"])
    click_word(session, "moved-shortcut", "Undo", minimum_y=90, maximum_y=145)
    wait_for(lambda: note_card()["rect"] == before, "one-step drag undo persisted")
    session.observe("undone", ["note.txt", "Redo"])
    click_word(session, "undone", "Redo", minimum_y=90, maximum_y=145)
    wait_for(lambda: note_card()["rect"] == moved, "drag redo persisted")
    session.observe("redone", ["note.txt", "Fit"])
    click_word(session, "redone", "note.txt", minimum_y=150)
    session.observe("selected-shortcut", ["note.txt", "Label"])
    handle = resize_handle(session, "selected-shortcut")
    drag(session, handle, (handle[0]+30,handle[1]+25))
    wait_for(lambda: note_card()["rect"][2] > moved[2]+20 and note_card()["rect"][3] > moved[3]+15, "live shortcut resize persisted")
    moved = note_card()["rect"][:]
    session.observe("resized-shortcut", ["note.txt", "Projects"])
    # Focus empty paper, select the existing pencil tool, and draw inside the
    # section body. A live section must not block drawing through its interior.
    session.click(1100, 650)
    session.key("p")
    drag(session, (820, 650), (940, 700))
    wait_for(lambda: any(e.get("type") == "freedraw" for e in json.loads(saved()["board"]["drawing"])["elements"]), "native pencil stroke saved")
    session.observe("native-pencil", ["Projects", "note.txt", "Link drawing to"])
    click_word(session, "native-pencil", "Link", minimum_y=140, maximum_y=220)
    session.observe("link-menu", ["note.txt", "fixture"])
    click_word(session, "link-menu", "note.txt", minimum_y=160, maximum_y=300)
    wait_for(lambda: any(b["pin"] == ids[0] for b in saved()["board"]["bindings"]), "drawing shortcut binding saved")
    view = saved()["board"]["view"]
    assert view and view["scale"] > 0
    session.click(1100, 650)
    session.key("t")
    session.click(1050, 620)
    session.observe("unsubmitted-text", ["Apply text", "Cancel"])
    private.run_checked(["wtype", "Keep this draft"], env=session.env)
    session.key("q", "ctrl")
    assert session.process.poll() is None, "closing must preserve an unapplied text draft"
    session.observe("unapplied-close", ["Unapplied drawing text", "Discard unapplied text"])
    click_word(session, "unapplied-close", "Discard", minimum_y=170, maximum_y=210)
    session.observe("draft-discarded", ["Projects", "note.txt"])
    click_word(session, "draft-discarded", "Arrange", minimum_y=90, maximum_y=145)
    session.observe("browse-before-pan", ["Projects", "note.txt"])
    drag(session, (1100,650), (1130,630))
    private.run_checked([str(session.pointer), "scroll", *map(str, session.logical_size), "1100", "650", "40"], env=session.env)
    wait_for(lambda: saved()["board"]["view"] != view and saved()["board"]["view"]["scale"] != view["scale"], "panned and zoomed camera saved")
    view = saved()["board"]["view"]
    session.observe("camera-changed", ["Projects", "note.txt"])
    session.close()
    session.restart(binary, root)
    session.observe("restarted-folder", ["note.txt", "Home"])
    enter_home(session, "restarted-folder")
    session.observe("restored-desk", ["Projects", "note.txt", "fixture"])
    assert [pin["id"] for pin in saved()["pins"]] == ids
    assert note_card()["rect"] == moved
    assert saved()["board"]["view"] == view, "restart must retain the authored camera, not fit again"
    assert any(e.get("type") == "freedraw" for e in json.loads(saved()["board"]["drawing"])["elements"])
    stroke = next(e for e in json.loads(saved()["board"]["drawing"])["elements"] if e.get("type") == "freedraw")
    target = []
    def painted_ink():
        point = artwork_target(session, "shortcut-ink", stroke, view)
        if point: target[:] = point
        return point
    wait_for(painted_ink, "current native pencil ink under the browse camera")
    session.click(*target)
    session.observe("pinned-text-studio", ["Back to folder", "Home fixture text"])
    session.key("Escape")
    session.observe("studio-return", ["note.txt", "List", "Home"])
    enter_home(session, "studio-return")
    session.observe("before-unpin", ["Projects", "note.txt"])
    x, y = session.word("before-unpin", "note.txt", minimum_y=140)
    private.run_checked([str(session.pointer), "right-click", *map(str, session.logical_size), str(x + 20), str(y)], env=session.env)
    session.observe("reference-menu", ["Unpin reference"])
    click_word(session, "reference-menu", "Unpin", minimum_y=140)
    wait_for(lambda: len(saved()["pins"]) == 1, "reference removal persisted")
    assert (root / "note.txt").read_text() == "Home fixture text\n"
    session.observe("folder-only", ["fixture", "Projects", "Pinned references: 1"])
    click_word(session, "folder-only", "fixture", minimum_y=140)
    session.observe("pinned-folder-opened", ["note.txt", "List", "Home"])
    enter_home(session, "pinned-folder-opened")
    session.observe("before-home-escape", ["Projects", "Back to folder"])
    session.key("Escape")
    session.observe("home-escape", ["note.txt", "List", "Home"])
    assert sorted(p.name for p in root.iterdir()) == ["note.txt"], "Home must not create content sidecars"


if __name__ == "__main__":
    args = application_arguments(__doc__, Path("target/debug/clip-file-manager"))
    output, root = media_fixture_directory(args)
    (root / "note.txt").write_text("Home fixture text\n")
    raise SystemExit(run_application_acceptance(args, output, root,
        lambda session, root: exercise(session, root, args.binary),
        "Native Home sections, arrangement undo/redo, pencil artwork, persistence, file/folder references and non-destructive unpin"))
