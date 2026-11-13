#!/usr/bin/env python3
"""Exercise native Mermaid flowchart editing on the private Wayland desktop.

Run once with --appearance light and once with --appearance dark, using distinct
--output-dir paths. Screenshots show real rendered nodes and native controls;
saved fixture bytes prove every edit and exact shared-session undo.
"""
from pathlib import Path

from diagram_vector_acceptance import color_bounds
from constellation_pitch_acceptance import control as isolated_control
from file_manager_acceptance import (
    application_arguments,
    media_fixture_directory,
    private,
    run_application_acceptance,
    wait_for,
)

SOURCE = (Path(__file__).resolve().parents[1] /
          "docs/examples/diagrams/mermaid-editor.mermaid").read_bytes()
SEQUENCE = b"sequenceDiagram\n  participant Dana\n  participant Eli\n  Dana->>Eli: Hello\n"


def control(session, shot, word, **options):
    point = session.word(shot, word, required=False, **options)
    if point is None:
        point = session.word(shot, word, tree=True, **options)
    session.click(point[0] + 4, point[1])


def open_fixture(session, name, shot):
    session.observe(shot + "-folder", [name])
    control(session, shot + "-folder", name)
    session.key("Return")


def canvas_observation(session, name, expected):
    """Wait for colored canonical SVG artwork, rather than source text matches."""
    # Fit once, including immediately after Save while rendering may be pending.
    # Production retains that request until current-source geometry arrives.
    session.click(241, 260)
    def rendered():
        session.observe(name, ["Flowchart editor"], timeout_seconds=45)
        try:
            color_bounds(session.output / f"{name}.png")
            color_bounds(session.output / f"{name}.png", (211, 249, 216))
        except AssertionError:
            return False
        session.observe(name, ["Flowchart editor", *expected], timeout_seconds=45)
        return True
    wait_for(rendered, "both authored Mermaid nodes rendered on the native canvas")


def select_blue_node(session, shot):
    bounds, _ = color_bounds(session.output / f"{shot}.png")
    session.click((bounds[0] + bounds[2]) // 2, (bounds[1] + bounds[3]) // 2)


def replace_field(session, x, y, text):
    session.click(x, y)
    session.key("a", "ctrl")
    private.run_checked(["wtype", text], env=session.env)


def save_exact(session, path, expected, reason):
    session.key("s", "ctrl")
    wait_for(lambda: path.read_bytes() == expected, reason)


def undo_exact(session, path, expected, reason):
    # An actual canvas click gives the shared session undo ownership rather
    # than undoing text typed into the inspector's local label field.
    session.click(430, 300)
    session.key("z", "ctrl")
    save_exact(session, path, expected, reason)


def label_edit(session, path):
    select_blue_node(session, "02-canvas")
    session.observe("03-node-selected", ["Shape:", "Apply label", "Connection label"])
    _, y = session.word("03-node-selected", "Label", minimum_y=300)
    replace_field(session, 1040, y + 28, "Plan")
    session.observe("04-label-entered", ["Plan", "Apply label"])
    control(session, "04-label-entered", "Apply", minimum_y=300)
    changed = SOURCE.replace(b'["Prepare"]', b'["Plan"]', 1)
    # Saving immediately after acceptance exercises the existing deferred save
    # path while the source-preserving edit is running on its bounded worker.
    save_exact(session, path, changed, "canvas label edit reaches guarded Save")
    canvas_observation(session, "05-label-saved", ["Plan", "Publish"])
    undo_exact(session, path, SOURCE, "Undo restores all original Mermaid bytes")
    canvas_observation(session, "06-label-undone", ["Prepare", "Publish"])


def add_and_connect(session, path):
    # Source is hidden, so the Add step inspector is on the right of the full
    # native canvas in the harness's fixed 1280 x 800 client.
    _, y = session.word("06-label-undone", "step", minimum_y=250)
    replace_field(session, 1040, y + 27, "Review")
    session.click(430, 300)  # Remove the text caret before OCR of the entered label.
    session.observe("07-add-ready", ["Review", "Add step"])
    # Light-theme OCR omits this bordered button even in the inspector crop;
    # the screenshot records the visible control and saved bytes prove its action.
    session.click(973, 398)
    added = SOURCE + b'node_1["Review"];\n'
    save_exact(session, path, added, "Add node appends one ordinary Mermaid declaration")
    canvas_observation(session, "08-node-added", ["Review", "Publish"])

    select_blue_node(session, "08-node-added")
    # One normal inspector scroll reveals Connect below the label fields.
    # The existing isolated-control locator handles its pale light-theme border.
    private.run_checked([str(session.pointer), "scroll", "1280", "800",
                         "1100", "600", "150"], env=session.env)
    session.observe("09-connect-source", ["Connection label"])
    session.click(*isolated_control(session, "09-connect-source", "Connect",
                                   (930, 284, 1180, 654)))
    session.scroll_to_visible("10-connect-destination", ["Connect from prepare"],
                              position=(1100, 600))
    # Review occurs on the canvas before the inspector's node list. Selecting
    # its rendered label exercises actual renderer geometry hit testing.
    control(session, "10-connect-destination", "Review", minimum_y=300)
    connected = added + b"prepare --> node_1;\n"
    save_exact(session, path, connected, "canvas destination creates a real source connection")
    canvas_observation(session, "11-connected", ["Prepare", "Review", "Publish"])
    undo_exact(session, path, added, "connection Undo preserves the added node and original source")
    canvas_observation(session, "12-connection-undone", ["Review", "Publish"])
    undo_exact(session, path, SOURCE, "node Undo restores exact original source bytes")
    canvas_observation(session, "13-original-restored", ["Prepare", "Publish"])


def source_only_family(session, root):
    session.click(1120, 71)  # Visible Back to folder in the native studio header.
    open_fixture(session, "b-sequence.mermaid", "14")
    session.observe("15-sequence-source", ["sequenceDiagram", "Dana", "Eli",
                    "Graphical editing currently supports Mermaid flowcharts"], timeout_seconds=45)
    # Unsupported graphical families retain the normal highlighted source
    # editor and live preview. Use its actual source field and shared Save.
    changed = SEQUENCE.replace(b"Hello", b"Welcome")
    # The highlighted keyword's OCR case differs by theme; target its visible
    # source field directly in the fixed private studio, then prove saved bytes.
    replace_field(session, 200, 294, changed.decode())
    path = root / "b-sequence.mermaid"
    save_exact(session, path, changed, "unsupported graphical family remains source editable")
    session.observe("16-sequence-edited", ["Welcome", "Graphical editing currently supports"])
    # Leave text-field undo explicitly and invoke the visible shared Undo
    # button, whose saved transaction restores the exact original source.
    # Full-window OCR omits this small bordered control in dark mode. Its
    # visible position follows the two-line source-only notice in this fixture.
    session.click(125, 747)
    save_exact(session, path, SEQUENCE, "source-only family shares exact source Undo and Save")
    session.observe("17-sequence-restored", ["Hello", "Graphical editing currently supports"])


def exercise(session, root):
    path = root / "a-flowchart.mermaid"
    open_fixture(session, path.name, "00")
    session.observe("01-source-and-preview", ["Flowchart editor", "Prepare", "Publish"],
                    timeout_seconds=45)
    # OCR merges the checked glyph with "Source". This visible checkbox has
    # the same fixed toolbar position in both themes on the private client.
    session.click(277, 260)
    canvas_observation(session, "02-canvas", ["Prepare", "Publish"])
    label_edit(session, path)
    add_and_connect(session, path)
    source_only_family(session, root)
    assert path.read_bytes() == SOURCE
    assert (root / "b-sequence.mermaid").read_bytes() == SEQUENCE


def main():
    args = application_arguments(__doc__, Path("target/debug/clip-file-manager"))
    output, root = media_fixture_directory(args)
    (root / "a-flowchart.mermaid").write_bytes(SOURCE)
    (root / "b-sequence.mermaid").write_bytes(SEQUENCE)
    return run_application_acceptance(args, output, root, exercise,
                                      "native Mermaid flowchart canvas and source-only family editing")


if __name__ == "__main__":
    raise SystemExit(main())
