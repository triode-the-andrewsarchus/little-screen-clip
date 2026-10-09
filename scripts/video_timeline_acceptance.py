#!/usr/bin/env python3
"""Verify progressive native-resolution/decimated strips on a private 4K desktop."""
import argparse
import json
from itertools import groupby
from pathlib import Path
import subprocess
from PIL import Image, ImageChops
import ui_acceptance_capture as private
from file_manager_acceptance import preserve_file_clipboard, wait_for


def painted_strip(path):
    with Image.open(path) as image:
        rgb = image.convert("RGB")
        red, green, blue = rgb.split()
        high = ImageChops.lighter(ImageChops.lighter(red, green), blue)
        low = ImageChops.darker(ImageChops.darker(red, green), blue)
        data = ImageChops.subtract(high, low).point(lambda value: 255 if value > 80 else 0).tobytes()
        counts = [sum(value != 0 for value in data[y * image.width:(y + 1) * image.width])
                  for y in range(image.height)]
        bands = [list(group) for saturated, group in
                 groupby(enumerate(counts), lambda pair: pair[1] > 2700) if saturated]
        bands = [band for band in bands if len(band) >= 32]
        assert bands, f"full-width colored timeline not painted: {max(counts)} pixels"
        band = max(bands, key=len)
        y = band[len(band) // 2][0]
        row = data[y * image.width:(y + 1) * image.width]
        runs = [list(group) for colored, group in
                groupby(enumerate(row), lambda pair: pair[1] != 0) if colored]
        strip = max(runs, key=len)
        return {"left": strip[0][0], "right": strip[-1][0], "y": y,
                "colored_pixels": len(strip)}



def helper_wrapper(path):
    # Keep completion pending until the private desktop has painted a snapshot.
    # The tiny native fixture can finish before the app's first 50 ms poll.
    path.write_text("""#!/usr/bin/env python3
import json,os,sys,subprocess,time
from pathlib import Path
with open(os.environ['FILES_TEST_WIDTH_LOG'],'a') as f:
    f.write(json.dumps(sys.argv[1:])+'\\n')
result = subprocess.run([os.environ['FILES_TEST_NATIVE']]+sys.argv[1:], timeout=10)
if result.returncode:
    sys.exit(result.returncode)
preview = Path(sys.argv[sys.argv.index('--progressive')+1])
if not preview.exists():
    temporary = preview.with_suffix('.tmp')
    temporary.write_bytes(Path(sys.argv[2]).read_bytes())
    temporary.replace(preview)
while not Path(os.environ['FILES_TEST_PROGRESS_GATE']).exists():
    time.sleep(.05)
sys.exit(0)
""")
    path.chmod(0o700)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output-dir", type=Path, required=True)
    parser.add_argument("--binary", type=Path, default=Path("target/release/clip-file-manager"))
    args = parser.parse_args()
    output = args.output_dir.resolve()
    output.mkdir(mode=0o700, parents=True, exist_ok=False)
    folder = output / "videos"
    folder.mkdir()
    subprocess.run(["ffmpeg", "-v", "error", "-f", "lavfi", "-i",
                    "color=black:size=1920x64:rate=128/601,geq=r='mod(N,3)*100':g='mod(N+1,3)*100':b='mod(N+2,3)*100'",
                    "-frames:v", "128", "-c:v", "libx264", "-qp", "0", "-g", "1",
                    "-threads", "2", "-pix_fmt", "yuv420p", str(folder / "colors.mp4")], check=True)
    runner_args = private.create_parser().parse_args([
        "--output-dir", str(output), "--width", "3840", "--height", "2160",
        "--appearance", "dark", "--allow-server-decorations"])
    config = private.prepare_configuration(runner_args, output)
    with config["niri_config"].open("a") as file:
        file.write('window-rule { match app-id="little-clip-file-manager"; open-floating true; }\n')
    children = private.ChildProcesses()
    observations = []
    try:
        _, env, _ = private.start_compositors(config, runner_args, output, children)
        private.verify_private_wayland_target(env, config["runtime"])
        private.APP_ID = "little-clip-file-manager"
        helper = Path("target/tools/video-strip/video-strip").resolve()
        wrapper = output / "helper.py"
        helper_wrapper(wrapper)
        env |= {"LITTLE_CLIP_VIDEO_STRIP": str(wrapper), "FILES_TEST_NATIVE": str(helper)}
        for factor in [1, 16]:
            width_log = output / f"width-{factor}.jsonl"
            env["FILES_TEST_WIDTH_LOG"] = str(width_log)
            gate = output / f"release-{factor}"
            env["FILES_TEST_PROGRESS_GATE"] = str(gate)
            text = f'[workbench]\nappearance="dark"\n[file_manager]\nvideo_timeline_decimation={factor}\n'
            (config["scratch"] / "config.toml").write_text(text)
            (Path(env["XDG_CONFIG_HOME"]) / "little-screen-clip/config.toml").write_text(text)
            process = children.start(f"files-{factor}", [str(args.binary.resolve()), str(folder)],
                                     env=env, cwd=config["scratch"], log_path=output / f"files-{factor}.log")
            private.wait_for_app_window(config["niri_bin"], env, process, output)
            private.run_checked([str(config["niri_bin"]), "msg", "action", "set-window-width", "3500"], env=env)
            private.run_checked([str(config["niri_bin"]), "msg", "action", "move-floating-window", "-x", "150", "-y", "400"], env=env)
            wait_for(lambda: width_log.exists() and any(int(json.loads(line)[2]) >= 3000 / factor
                     for line in width_log.read_text().splitlines()), "4K target-width helper request")
            screenshot = output / f"strip-{factor}.png"
            painted = None
            def capture_ready():
                nonlocal painted
                with preserve_file_clipboard(env):
                    private.screenshot(config["niri_bin"], env, screenshot)
                try:
                    painted = painted_strip(screenshot)
                    widths = [int(json.loads(line)[2]) for line in width_log.read_text().splitlines()]
                    # Niri resize transitions can paint a transformed older
                    # surface after Files has already requested its final width.
                    return abs(max(widths) * factor - painted["colored_pixels"]) <= factor + 4
                except AssertionError:
                    return False
            wait_for(capture_ready, "full-width snapshot painting before helper completion")
            assert not gate.exists(), "Render completion was released before snapshot painting"
            widths = [int(json.loads(line)[2]) for line in width_log.read_text().splitlines()]
            assert abs(max(widths) * factor - (painted["right"] - painted["left"] + 1)) <= factor + 4
            observations.append({"factor": factor, "requested_columns": widths,
                                 "painted": painted, "screenshot": str(screenshot)})
            gate.touch()
            private.run_checked([str(config["niri_bin"]), "msg", "action", "close-window"], env=env)
            process.wait(timeout=20)
            assert process.returncode == 0
        (output / "report.json").write_text(json.dumps({"status": "passed", "observations": observations}, indent=2))
        print(json.dumps(observations))
    finally:
        children.cleanup()


if __name__ == "__main__":
    main()
