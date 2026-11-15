#!/usr/bin/env python3
"""Exercise a real Copy-only artwork drop from one native Files client to Home."""
import json
from collections import Counter
from pathlib import Path
from PIL import Image
from file_manager_acceptance import Session, application_arguments, finish_run, wait_for
from file_manager_desktop_acceptance import drag, position, start_two_clients
from file_manager_home_acceptance import click_cropped_word, resize_handle
import ui_acceptance_capture as private


if __name__ == "__main__":
    args = application_arguments(__doc__, Path("target/debug/clip-file-manager"), lambda parser: parser.add_argument("--note", action="store_true"))
    output = args.output_dir.resolve()
    output.mkdir(mode=0o700)
    children = private.ChildProcesses()
    report = {"status": "running", "appearance": args.appearance}
    try:
        source = output / "SOURCE"
        target = output / "DESTINATION"
        source.mkdir(); target.mkdir()
        icon = source / ("neutral.note" if args.note else "neutral.svg")
        color = "#181008" if args.appearance == "dark" else "#f7efe7"
        original = f'<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"><path d="M10 50L90 50" fill="none" stroke="{color}" stroke-width="5"/></svg>'.encode()
        if args.note:
            original = (Path(__file__).resolve().parents[1] / "docs/examples/notebooks/supernote/demo-a5x-20230015-1to10.note").read_bytes()
        icon.write_bytes(original)
        config, env, apps = start_two_clients(output, args.appearance, args.binary, [source, target], children)
        session = Session(output, env, config["niri_bin"], apps[1], children)
        session.ocr_regions = [("home-right-controls-ocr.png", (650,78,1270,136))]
        session.observe("two-clients", ["neutral"])
        if not click_cropped_word(session, "two-clients", "Home", (1058,78,1123,110), 65, 120):
            # Observed native 640px client geometry; dark bordered text can be omitted by OCR.
            session.click(1090, 94)
        session.observe("home", ["neutral", "Arrange"])
        drag(session, position(session, "home", icon.name, True), (970, 450))
        home = Path(env["XDG_STATE_HOME"]) / "little-screen-clip/home.json"
        wait_for(lambda: home.exists() and any(card["content"]["kind"] == "artwork" for card in json.loads(home.read_text())["board"]["cards"]), "native Home artwork import committed")
        session.observe("imported", ["Pinned references: 1"])
        assert icon.read_bytes() == original, "Home drops must never remove or rewrite their source"
        assert not list(target.iterdir()), "Home imports must not become ordinary folder transfers"
        target_ink = lambda pixel: min(pixel) > 230 if args.appearance == "dark" else max(pixel) < 48
        def visible():
            session.capture_text("painted", output / "painted.png", [])
            with Image.open(output / "painted.png").convert("RGB") as screen:
                crop = screen.crop((860, 350, 1100, 550))
                count = sum(target_ink(pixel) for pixel in getattr(crop, "get_flattened_data", crop.getdata)())
            return (5 if args.note else 100) < count < 4000
        wait_for(visible, "native transparent vector ink, with no opaque backdrop")
        saved = json.loads(home.read_text())
        card = next(card for card in saved["board"]["cards"] if card["content"]["kind"] == "artwork")
        assert next(pin for pin in saved["pins"] if pin["id"] == card["content"]["pin"])["path"] == str(icon)
        # Artwork moves/resizes directly, without requiring Arrange mode.
        before = card["rect"][:]
        drag(session, (970,470), (1010,490))
        wait_for(lambda: json.loads(home.read_text())["board"]["cards"][0]["rect"][0] > before[0]+5, "actual artwork moved")
        session.observe("moved", ["Pinned references: 1"])
        moved = json.loads(home.read_text())["board"]["cards"][0]["rect"][:]
        handle = resize_handle(session, "moved", maximum_x=1270)
        drag(session, handle, (handle[0]+25,handle[1]+20))
        wait_for(lambda: json.loads(home.read_text())["board"]["cards"][0]["rect"][2] > moved[2]+5, "actual artwork resized")
        saved = json.loads(home.read_text())
        apps[0].terminate(); apps[0].wait(timeout=5)
        session.close()
        session.restart(args.binary)
        session.ocr_regions = [("home-controls-ocr.png", (185,98,880,138))]
        session.observe("restarted", ["Pinned references: 1"])
        def restored_ink():
            session.capture_text("restored-ink", output / "restored-ink.png", [])
            with Image.open(output / "restored-ink.png").convert("RGB") as screen:
                crop = screen.crop((100,180,1180,730))
                colors = Counter(pixel for pixel in getattr(crop, "get_flattened_data", crop.getdata)() if target_ink(pixel))
            if not colors:
                return False
            dominant, count = colors.most_common(1)[0]
            return count > (5 if args.note else 100) and (dominant[0] == dominant[1] == dominant[2] if args.note else dominant[0] > dominant[1] > dominant[2])
        wait_for(restored_ink, "restored vector ink with preserved warm hue")
        assert json.loads(home.read_text())["board"]["cards"] == saved["board"]["cards"]
        assert icon.read_bytes() == original
        report.update(status="passed", observations=session.observations, private_runtime=str(config["runtime"]))
    except Exception as error:
        if "session" in locals() and session.process.poll() is None:
            session.observe("failure-state", [])
        report.update(status="failed", error=str(error), observations=session.observations if "session" in locals() else [])
    finally:
        result = finish_run(output, children, report)
    raise SystemExit(result)
