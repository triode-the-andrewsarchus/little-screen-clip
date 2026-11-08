"""Create owned native Krita fixtures through isolated command-line conversion."""
import io
import os
from pathlib import Path
import subprocess
import tempfile
import zipfile
from PIL import Image


def write(path, width=64, height=32, layered=True):
    """Build RGB panels (or a flat green canvas) and let Krita save its archive."""
    path = Path(path).resolve()
    with tempfile.TemporaryDirectory(prefix="kra-fixture-") as directory:
        root = Path(directory)
        source = root / ("source.ora" if layered else "source.png")
        if layered:
            markup = f'''<image w="{width}" h="{height}" name="Panels"><stack>
<stack name="Artwork"><layer name="Top patch" src="data/top.png" x="{width//2}" y="0" opacity="1" visibility="visible" composite-op="svg:src-over"/>
<layer name="Hidden tint α" src="data/hidden.png" x="-8" y="-4" opacity="0.5" visibility="hidden" composite-op="svg:multiply"/></stack>
<layer name="Background" src="data/background.png" x="0" y="0" opacity="1" visibility="visible" composite-op="svg:src-over"/></stack></image>'''
            with zipfile.ZipFile(source, "w") as archive:
                archive.writestr("mimetype", "image/openraster")
                archive.writestr("stack.xml", markup)
                for name, size, color in [
                    ("top", (width-width//2, height), (180, 60, 40, 255)),
                    ("hidden", (width+16, height+8), (50, 200, 20, 255)),
                    ("background", (width, height), (30, 120, 210, 255)),
                ]:
                    png = io.BytesIO()
                    Image.new("RGBA", size, color).save(png, format="PNG")
                    archive.writestr(f"data/{name}.png", png.getvalue())
                composite = Image.new("RGBA", (width, height), (30, 120, 210, 255))
                composite.paste((180, 60, 40, 255), (width//2, 0, width, height))
                png = io.BytesIO()
                composite.save(png, format="PNG")
                archive.writestr("mergedimage.png", png.getvalue())
        else:
            Image.new("RGB", (width, height), (90, 170, 70)).save(source)
        env = os.environ.copy()
        for key in ("DISPLAY", "WAYLAND_DISPLAY", "QT_QPA_PLATFORMTHEME"):
            env.pop(key, None)
        env.update(QT_QPA_PLATFORM="offscreen", XDG_CONFIG_HOME=str(root/"config"), XDG_DATA_HOME=str(root/"data"))
        result = subprocess.run(["dbus-run-session", "--", "krita", str(source), "--export", "--export-filename", str(path)],
            env=env, capture_output=True, text=True, timeout=60)
        if result.returncode or not path.is_file():
            raise RuntimeError(f"Krita fixture conversion failed: {result.stderr}")


if __name__ == "__main__":
    root = Path(__file__).resolve().parent.parent / "crates/little-clip-core/tests/fixtures"
    write(root / "layered.kra")
    write(root / "flat.kra", layered=False)
