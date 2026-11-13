#!/usr/bin/env python3
"""Exercise WireViz terminal wiring on the existing private Wayland desktop.

Run rebuilt apps in light/dark with separate output directories. This script
never builds Rust or modifies the ordinary compositor. Native pointer/OCR,
guarded Save and shared Undo provide the evidence; existing source bytes and
all unaffected YAML/circuit data are checked independently. New connection-set
formatting is retained as an exact saved-byte snapshot for Undo verification.

The electrical oracle deliberately covers only this fixture's typed pin IDs,
numbered/shield conductors, numeric ranges, duplicates and connector loops. It
is not a second WireViz parser. Mating editing and template expansion remain
explicitly unsupported in the application and outside this scenario.

`--self-test` exercises only the bounded fixture/source/circuit oracle; it does
not launch a client, compositor or Rust command.
"""
import copy
from collections import Counter
from contextlib import contextmanager
import hashlib
import json
from pathlib import Path
import re
import subprocess
import sys

from PIL import Image, ImageOps
import yaml

from constellation_pitch_acceptance import control as isolated_control
from diagram_wireviz_acceptance import has_rendered_headers
from file_manager_acceptance import (
    application_arguments, media_fixture_directory, private,
    run_application_acceptance, wait_for,
)

FIXTURE = Path(__file__).resolve().parents[1] / "docs/examples/diagrams/harness-wiring-acceptance.wireviz"
SOURCE = FIXTURE.read_bytes()
TAIL = b"# Preserve this opaque tail byte for byte, including the alias and block scalar.\n"
PREFIX, SUFFIX = SOURCE.split(TAIL, 1)
SUFFIX = TAIL + SUFFIX
ORIGINAL = yaml.safe_load(SOURCE)
CANVAS = (100, 280, 915, 690)
INSPECTOR = (920, 280, 1180, 690)
LABELS = ["PROBE"]


def identity(value):
    assert type(value) in (int, str), f"fixture identity must be a typed integer/string: {value!r}"
    return type(value).__name__, value


def selections(value):
    if not isinstance(value, list):
        return [value]
    output = []
    for item in value:
        match = re.fullmatch(r"(\d+)-(\d+)", item) if isinstance(item, str) else None
        if match:
            first, last = map(int, match.groups())
            assert abs(last - first) < 512
            output.extend(range(first, last + (1 if first <= last else -1), 1 if first <= last else -1))
        else:
            output.append(item)
    return output


def terminal(document, component, selector):
    if component in document["connectors"]:
        attrs = document["connectors"][component]
        pins = attrs.get("pins", list(range(1, attrs.get("pincount", 0) + 1)))
        assert identity(selector) in [identity(pin) for pin in pins]
        return "pin", component, *identity(selector)
    attrs = document["cables"][component]
    count = attrs.get("wirecount", len(attrs.get("colors", [])))
    assert (type(selector) is int and 1 <= selector <= count) or (selector == "s" and attrs.get("shield"))
    return "wire", component, *identity(selector)


def endpoints(document, step, used):
    if isinstance(step, str):
        used.add(step)
        if step in document["cables"]:
            return []  # Sole cable designator: visible component, no invented wire.
        attrs = document["connectors"][step]
        pins = attrs.get("pins", list(range(1, attrs.get("pincount", 0) + 1)))
        assert len(pins) == 1, "fixture oracle does not implement connector-body mating"
        return [terminal(document, step, pins[0])]
    assert isinstance(step, dict) and len(step) == 1, "fixture oracle accepts only ordinary electrical steps"
    component, selector = next(iter(step.items()))
    used.add(component)
    return [terminal(document, component, value) for value in selections(selector)]


def edge(kind, first, second):
    return kind, *sorted((first, second), key=repr)


def circuit(document):
    links, used = Counter(), set()
    for path in document["connections"]:
        assert 1 <= len(path) <= 256
        columns = [endpoints(document, step, used) for step in path]
        for left, right in zip(columns, columns[1:]):
            assert len(left) == len(right) and left
            for first, second in zip(left, right):
                assert {first[0], second[0]} == {"pin", "wire"}
                links[edge("cable", first, second)] += 1
    for name, attrs in document["connectors"].items():
        if name in used:
            for first, second in attrs.get("loops", []):
                links[edge("loop", terminal(document, name, first), terminal(document, name, second))] += 1
    return links, used


ORIGINAL_LINKS, ORIGINAL_USED = circuit(ORIGINAL)
CONNECTED_VALUE = copy.deepcopy(ORIGINAL)
CONNECTED_VALUE["connections"].append([{"P": "P01"}, {"LOOM": 2}])
CONNECT_EDGE = edge("cable", terminal(ORIGINAL, "P", "P01"), terminal(ORIGINAL, "LOOM", 2))
PARALLEL_EDGE = edge("cable", terminal(ORIGINAL, "INPUT", 2), terminal(ORIGINAL, "LOOM", 2))


def disconnected_probe_value():
    document = copy.deepcopy(ORIGINAL)
    document["connections"].extend([[{"P": "P01"}], [{"LOOM": 2}]])
    return document


def disconnected_parallel_value():
    document = copy.deepcopy(ORIGINAL)
    document["connections"][0] = [{"INPUT": ["01", 3]}, {"LOOM": [1, 3]}, {"OUTPUT": [3, 1]}]
    document["connections"].extend([[{"INPUT": 2}], [{"LOOM": 2}, {"OUTPUT": 2}]])
    return document


def parallel_prefix():
    return PREFIX.replace(b'INPUT: ["01", 2, 3] #', b'INPUT: ["01", 3] #', 1).replace(
        b'LOOM: [1-3]', b'LOOM: [1, 3]', 1).replace(b'OUTPUT: [3-1]', b'OUTPUT: [3, 1]', 1)


def prove(data, expected, expected_links, prefix=PREFIX):
    """Only precise selector edits plus newly appended sets may alter bytes."""
    assert len(data) <= 256 * 1024
    assert data.startswith(prefix), "existing declarations, selectors/comments or untouched sets changed"
    assert data.endswith(SUFFIX), "opaque YAML, alias spelling or block scalar changed"
    document = yaml.safe_load(data)
    assert document == expected, "complete saved YAML differs outside the planned change"
    links, used = circuit(document)
    assert links == expected_links, f"electrical link multiset differs: {links - expected_links}; missing {expected_links - links}"
    assert used == ORIGINAL_USED, "disconnect must retain explicit singleton visibility"
    return document


def save_checked(session, path, expected, expected_links, shot, prefix=PREFIX):
    session.key("s", "ctrl")
    result, failure = [], []
    def accepted():
        data = path.read_bytes()
        try:
            document = prove(data, expected, expected_links, prefix)
        except (AssertionError, yaml.YAMLError) as error:
            failure[:] = [str(error)]
            return False
        result[:] = [data, document]
        return True
    try:
        wait_for(accepted, f"{shot}: guarded save matches precise source and electrical proof")
    except RuntimeError as error:
        raise AssertionError(f"{error}; last proof failure: {failure}") from error
    data, document = result
    (session.output / f"{shot}.wireviz").write_bytes(data)
    (session.output / f"{shot}-proof.json").write_text(json.dumps({
        "sha256": hashlib.sha256(data).hexdigest(), "used_components": sorted(ORIGINAL_USED),
        "links": [{"kind": kind, "from": first, "to": second, "count": count}
                  for (kind, first, second), count in sorted(circuit(document)[0].items(), key=repr)],
        "unchanged_prefix_bytes": len(prefix), "unchanged_opaque_tail_bytes": len(SUFFIX),
    }, indent=2) + "\n")
    return data


def canvas(session, shot):
    def ready():
        text = session.observe(shot, ["Harness explorer", *LABELS], timeout_seconds=45)
        return has_rendered_headers(session.output / f"{shot}.png") and "updatingharness" not in "".join(text.lower().split())
    wait_for(ready, "fresh native harness tables and terminal labels", timeout_seconds=60)


def locate_terminal(session, shot, label):
    """Read only painted canvas text; inspector/source labels cannot be targets."""
    if label in {"RED", "GREEN"}:
        return conductor_band(session, shot, (255, 0, 0) if label == "RED" else (0, 255, 0))
    with Image.open(session.output / f"{shot}.png").convert("RGB") as image:
        crop = ImageOps.autocontrast(image.crop(CANVAS).convert("L"))
        isolated = Image.new("L", image.size, 255)
        isolated.paste(crop, CANVAS[:2])
        isolated.resize((image.width * 3, image.height * 3)).save(session.output / f"{shot}-terminals-ocr.png")
    name = shot + "-terminals"
    point = (session.word(name, label, required=False, minimum_y=280) or
             session.word(name, label, tree=True, minimum_y=280))
    assert CANVAS[0] <= point[0] <= CANVAS[2] and CANVAS[1] <= point[1] <= CANVAS[3]
    return point[0] + 4, point[1]


def conductor_band(session, shot, color):
    """Target the real authored conductor swatch, avoiding tiny-label OCR."""
    with Image.open(session.output / f"{shot}.png").convert("RGB") as image:
        pixels = image.load()
        pending = {(x, y) for y in range(CANVAS[1], CANVAS[3]) for x in range(CANVAS[0], CANVAS[2])
                   if all(abs(first - second) <= 2 for first, second in zip(pixels[x, y], color))}
    largest = []
    while pending:
        frontier, component = [pending.pop()], []
        while frontier:
            x, y = frontier.pop()
            component.append((x, y))
            for neighbor in ((x - 1, y), (x + 1, y), (x, y - 1), (x, y + 1)):
                if neighbor in pending:
                    pending.remove(neighbor)
                    frontier.append(neighbor)
        if len(component) > len(largest):
            largest = component
    assert len(largest) > 80, f"authored conductor band {color} must occupy real native artwork"
    return tuple(round(sum(point[axis] for point in largest) / len(largest)) for axis in (0, 1))


def drag(session, start, end):
    private.run_checked([str(session.pointer), "drag", "1280", "800", *map(str, (*start, *end))], env=session.env)


@contextmanager
def held_drag(session, start, end):
    process = subprocess.Popen([str(session.pointer), "drag-held", "1280", "800", *map(str, (*start, *end))],
                               env=session.env, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                               stderr=subprocess.PIPE, text=True)
    try:
        assert process.stdout.readline().strip() == "holding private drag"
        yield process
        assert process.poll() is None, "cancellation evidence must occur before the private pointer releases"
    finally:
        process.communicate("release\n", timeout=12)
        assert process.returncode == 0


def save_exact(session, path, data, reason):
    session.key("s", "ctrl")
    wait_for(lambda: path.read_bytes() == data, reason)


def focus_canvas(session):
    session.click(450, 295)  # Ordinary canvas paper above the fitted component rows.


def undo_exact(session, path, data, shot):
    focus_canvas(session)
    session.key("z", "ctrl")
    save_exact(session, path, data, "shared Undo and guarded Save restore every original source byte")
    canvas(session, shot)
    assert path.read_bytes() == data


def disconnect(session, shot):
    session.observe(shot, ["Direct connections", "Pin label"])
    point = (session.word(shot, "Disconnect", required=False, minimum_y=280) or
             isolated_control(session, shot, "Disconnect", INSPECTOR))
    assert INSPECTOR[0] <= point[0] <= INSPECTOR[2], "Disconnect must be the native inspector control"
    session.click(point[0] + 4, point[1])


def cancellations(session, path):
    start = locate_terminal(session, "02-canvas", "PROBE")
    incompatible = locate_terminal(session, "02-canvas", "FEED")
    destination = locate_terminal(session, "02-canvas", "RED")
    drag(session, start, incompatible)
    save_exact(session, path, SOURCE, "pin-to-pin drag cannot invent mating or wiring")
    canvas(session, "03-incompatible-cancelled")
    assert path.read_bytes() == SOURCE
    with held_drag(session, start, destination):
        assert path.read_bytes() == SOURCE, "held drag must not save source"
        session.key("Escape")
    save_exact(session, path, SOURCE, "Escape cancels a held valid connection drag")
    canvas(session, "04-escape-cancelled")
    assert path.read_bytes() == SOURCE


def connect_and_disconnect(session, path):
    start = locate_terminal(session, "04-escape-cancelled", "PROBE")
    destination = locate_terminal(session, "04-escape-cancelled", "RED")
    with held_drag(session, start, destination):
        session.observe("05-held-connect", ["Harness explorer", "PROBE"], preserve_pointer=True, timeout_seconds=5)
        assert path.read_bytes() == SOURCE, "live connection preview must not save before release"
        midpoint = tuple(round((a + b) / 2) for a, b in zip(start, destination))
        with Image.open(session.output / "04-escape-cancelled.png").convert("RGB") as before, \
                Image.open(session.output / "05-held-connect.png").convert("RGB") as during:
            x, y = midpoint
            changed = sum(max(abs(a - b) for a, b in zip(before.getpixel((px, py)), during.getpixel((px, py)))) > 40
                          for px in range(x - 4, x + 5) for py in range(y - 4, y + 5))
            assert changed >= 3, "native connection line must be painted between terminals while held"
    connected_links = ORIGINAL_LINKS + Counter({CONNECT_EDGE: 1})
    connected = save_checked(session, path, CONNECTED_VALUE, connected_links, "05-connected")
    canvas(session, "06-connected-canvas")
    # Undo while a different valid drag is physically held. Source/model changes
    # must cancel the captured endpoint revision before its release can submit.
    with held_drag(session, locate_terminal(session, "06-connected-canvas", "PROBE"),
                   locate_terminal(session, "06-connected-canvas", "GREEN")):
        session.key("z", "ctrl")
        save_exact(session, path, SOURCE, "Undo changes the exact source while the pointer remains held")
    save_exact(session, path, SOURCE, "stale connection drag cannot publish after Undo")
    canvas(session, "07-stale-cancelled")
    assert path.read_bytes() == SOURCE
    drag(session, locate_terminal(session, "07-stale-cancelled", "PROBE"),
         locate_terminal(session, "07-stale-cancelled", "RED"))
    repeated = save_checked(session, path, CONNECTED_VALUE, connected_links, "08-connected-again")
    assert repeated == connected, "repeated typed connection has deterministic exact source bytes"
    canvas(session, "09-disconnect-target")
    session.click(*locate_terminal(session, "09-disconnect-target", "PROBE"))
    disconnect(session, "10-probe-inspector")
    save_checked(session, path, disconnected_probe_value(), ORIGINAL_LINKS, "11-probe-disconnected")
    canvas(session, "12-probe-disconnected-canvas")
    undo_exact(session, path, connected, "13-disconnect-undone")
    undo_exact(session, path, SOURCE, "14-connect-undone")


def parallel_disconnect(session, path):
    session.click(*locate_terminal(session, "14-connect-undone", "FEED"))
    disconnect(session, "15-parallel-inspector")
    expected_links = ORIGINAL_LINKS - Counter({PARALLEL_EDGE: 1})
    save_checked(session, path, disconnected_parallel_value(), expected_links,
                 "16-parallel-disconnected", parallel_prefix())
    canvas(session, "17-parallel-disconnected-canvas")
    undo_exact(session, path, SOURCE, "18-original-restored")


def exercise(session, root):
    path = root / "a-wiring.wireviz"
    session.observe("00-folder", [path.name])
    point = session.word("00-folder", path.name)
    session.click(point[0] + 10, point[1])
    session.key("Return")
    canvas(session, "01-opened")
    session.click(248, 260)  # Existing native Harness explorer Fit control.
    canvas(session, "02-canvas")
    cancellations(session, path)
    connect_and_disconnect(session, path)
    parallel_disconnect(session, path)
    assert path.read_bytes() == SOURCE, "the final file must exactly match the authored fixture"


def self_test():
    assert sum(ORIGINAL_LINKS.values()) == 8 and len(ORIGINAL_USED) == 4
    assert circuit(CONNECTED_VALUE)[0] == ORIGINAL_LINKS + Counter({CONNECT_EDGE: 1})
    assert circuit(disconnected_probe_value()) == (ORIGINAL_LINKS, ORIGINAL_USED)
    assert circuit(disconnected_parallel_value()) == (ORIGINAL_LINKS - Counter({PARALLEL_EDGE: 1}), ORIGINAL_USED)
    prove(SOURCE, ORIGINAL, ORIGINAL_LINKS)
    # Preserve exact original bytes while accepting newly generated source only.
    addition = b"  - - P: P01\n    - LOOM: 2\n"
    changed = PREFIX + addition + SUFFIX
    prove(changed, CONNECTED_VALUE, ORIGINAL_LINKS + Counter({CONNECT_EDGE: 1}))
    mutations = [
        changed.replace(b'keep: "NO"', b'keep: changed'),
        changed.replace(b'opaque_alias: *future', b'opaque_alias:    *future'),
        changed.replace(b'# retain the duplicate occurrence', b'# edited comment'),
        changed.replace(b'P: P01', b'P: PROBE'),
        changed.replace(b'LOOM: 2\n', b'LOOM: 3\n'),
    ]
    for corrupted in mutations:
        try:
            prove(corrupted, CONNECTED_VALUE, ORIGINAL_LINKS + Counter({CONNECT_EDGE: 1}))
        except (AssertionError, yaml.YAMLError):
            pass
        else:
            raise AssertionError("source/circuit oracle must reject unrelated bytes or wrong terminal references")
    missing_duplicate = copy.deepcopy(ORIGINAL)
    missing_duplicate["connections"].pop(1)
    assert circuit(missing_duplicate)[0] != ORIGINAL_LINKS, "multiplicity cannot collapse to an endpoint set"
    missing_singleton = copy.deepcopy(ORIGINAL)
    missing_singleton["connections"].pop()
    assert circuit(missing_singleton)[1] != ORIGINAL_USED, "explicit unconnected component visibility is meaningful"
    print("WireViz fixture/source/electrical oracle passed; no GUI or Rust command was run")


def main():
    if sys.argv[1:] == ["--self-test"]:
        self_test()
        return 0
    args = application_arguments(__doc__, Path("target/debug/clip-file-manager"))
    output, root = media_fixture_directory(args)
    (root / "a-wiring.wireviz").write_bytes(SOURCE)
    return run_application_acceptance(args, output, root, exercise,
                                      "native WireViz terminal connect/disconnect, cancellation, source proof and exact Undo/Save")


if __name__ == "__main__":
    raise SystemExit(main())
