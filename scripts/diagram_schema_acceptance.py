#!/usr/bin/env python3
"""Verify schema artwork in native file strips, source and Markdown views."""
import hashlib
from pathlib import Path
from PIL import Image

from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance, wait_for
from text_strip_acceptance import hover_observation

EXAMPLES = Path(__file__).resolve().parents[1] / "docs/examples/diagrams"
FAMILIES = {
    'pikchr': ('Pikchr', 'pipeline.pikchr', (0, 0, 0), 'box at invalid nonsense'),
    'svgbob': ('svgbob', 'pipeline.svgbob', (0, 0, 0), '+' * 257),
    'seqdiag': ('SeqDiag', 'request.seqdiag', (217, 232, 251), 'seqdiag {'),
    'blockdiag': ('BlockDiag', 'pipeline.blockdiag', (217, 232, 251), 'blockdiag {'),
    'actdiag': ('ActDiag', 'review.actdiag', (217, 232, 251), 'actdiag {'),
    'nwdiag': ('NwDiag', 'service.nwdiag', (217, 232, 251), 'nwdiag {'),
    'packetdiag': ('PacketDiag', 'header.packetdiag', (217, 232, 251), 'packetdiag {'),
    'rackdiag': ('RackDiag', 'server.rackdiag', (217, 240, 223), 'rackdiag {'),
    'ditaa': ('Ditaa', 'pipeline.ditaa', (85, 85, 187), '@startditaa\n+---+'),
    'umlet': ('UMLet', 'umlet/class-relations.uxf', (244, 248, 255), '<diagram>'),
    'symbolator': ('Symbolator', 'entity.symbolator.vhdl', (231, 253, 186), 'entity missing is'),
    'tikz': ('TikZ / PGF', 'native-tikz.tikz', (234, 239, 245), '\\input{Cargo.toml}\\begin{tikzpicture}\\draw (0,0)--(1,1);\\end{tikzpicture}'),
    "vega": ("Vega", "native-vega.vega.json", (41, 95, 152), "{"),
    "vegalite": ("Vega-Lite", "native-vega-lite.vl.json", (218, 155, 66), "{"),
    "structurizr": ("Structurizr", "architecture.structurizr", (41, 95, 152), "workspace { model {"),
    "drawio": ("draw.io", "pipeline.drawio", (218, 232, 252), "<mxfile><diagram>broken!</diagram></mxfile>"),
    "nomnoml": ("Nomnoml", "pipeline.nomnoml", (144, 238, 144), "[Broken"),
    "dbml": ("DBML", "schema.dbml", (34, 56, 77), "Table broken {"),
    "goat": ("GoAT", "pipeline.goat", (17, 17, 17), "a\tb"),
    "bytefield": ("Bytefield", "packet.bytefield", (184, 216, 236), "(draw-box"),
    "erd": ("Erd", "people.er", (208, 224, 208), "[Person"),
    "plantuml": ("PlantUML", "sequence.puml", (184, 216, 236), "@startuml\nAlice -> Bob"),
    "puml": ("PlantUML", "architecture.puml", (8, 66, 123), "@startuml\n!include <Missing/Resource>\n@enduml"),
}


def artwork(path, color, minimum=200, bounds=None):
    with Image.open(path).convert("RGB") as image:
        # The fixture's node fill must occupy real painted area; source words
        # appearing in the syntax editor cannot prove that rendering worked.
        pixels = iter((image.crop(bounds) if bounds else image).tobytes())
        return sum(all(abs(a - b) < 3 for a, b in zip((r, g, b), color))
                   for r, g, b in zip(pixels, pixels, pixels)) > minimum


def rendered(session, name, expected, color, minimum=200):
    def ready():
        session.observe(name, expected, timeout_seconds=45)
        return artwork(session.output / f"{name}.png", color, minimum)
    wait_for(ready, "native schema node artwork")


def exercise(session, root):
    # Family filenames are verified inside each addressed folder below. The
    # initial light-theme folder labels are ambiguous to OCR (nomnoml/momnoml).
    session.observe("00-families", [f"{len(FAMILIES)} folder"])
    session.click(647, 160)  # Collapse Shelf once in the fixed private viewport.
    for syntax, (title, example, color, _) in FAMILIES.items():
        folder = root / syntax
        before = {p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in folder.iterdir()}
        session.address(folder)
        prefix = syntax + "-"
        valid, invalid = f"a-pipeline.{syntax}", f"b-invalid.{syntax}"
        rendered(session, prefix + "01-strips", [valid, invalid, "c-overview.md"], color, minimum=80 if syntax in {"symbolator", "seqdiag", "actdiag", "nwdiag", "packetdiag", "rackdiag"} else 500)
        _, y = session.word(prefix + "01-strips", valid)
        # These format names also appear in the row's type badge. Require
        # enlarged paper outside the row before capturing the hover evidence.
        enlarged = (lambda path: artwork(path, (255, 255, 255), 1000,
                                         bounds=(230, 320, 325, 650))) if syntax in {"pikchr", "svgbob"} else None
        hover_observation(session, prefix + "02-popout", 700, y + 42, [title], rendered=enlarged)
        assert artwork(session.output / f"{prefix}02-popout.png", color), f"{syntax}: authored artwork missing from hover"
        _, y = session.word(prefix + "01-strips", invalid)
        hover_observation(session, prefix + "03-invalid", 700, y + 42, ["Rendered preview unavailable"])
        x, y = session.word(prefix + "01-strips", valid)
        session.click(x + 20, y)
        session.key("Return")
        rendered(session, prefix + "04-source", ["Diagram", "Preview"], color)
        session.click(1120, 71)
        session.observe(prefix + "05-return", ["c-overview.md"])
        x, y = session.word(prefix + "05-return", "c-overview.md")
        session.click(x + 20, y)
        session.key("Return")
        rendered(session, prefix + "06-markdown", ["Schema overview"], color)
        # Tall diagrams can legitimately place the source action below the
        # viewport. Verify artwork first, then scroll to the source affordance.
        session.scroll_to_visible(prefix + "06-markdown-source", ["Diagram source"])
        session.click(1120, 71)
        session.observe(prefix + "07-folder", [valid, "c-overview.md"])
        after = {p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in folder.iterdir()}
        assert before == after, "passive rendering must preserve all source files"


if __name__ == "__main__":
    args = application_arguments(
        __doc__, Path("target/debug/clip-file-manager"),
        configure=lambda parser: parser.add_argument("--family", action="append", choices=list(FAMILIES)),
    )
    if args.family:
        FAMILIES = {name: FAMILIES[name] for name in args.family}
    output, root = media_fixture_directory(args)
    for syntax, (_, example, _, invalid) in FAMILIES.items():
        folder = root / syntax
        folder.mkdir()
        source = (EXAMPLES / example).read_text()
        (folder / f"a-pipeline.{syntax}").write_text(source)
        (folder / f"b-invalid.{syntax}").write_text(invalid)
        (folder / "c-overview.md").write_text("# Schema overview\n\n```" + syntax + "\n" + source + "```\n")
    raise SystemExit(run_application_acceptance(args, output, root, exercise, "native schema diagram previews"))
