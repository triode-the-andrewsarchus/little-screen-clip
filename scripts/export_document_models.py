#!/usr/bin/env python3
"""Reproduce the bundled UVDoc/SyntheticDoc ONNX graphs (development only).

Requires torch, onnx and onnxruntime. No Python packages are needed by the app.
Downloads are pinned by revision and SHA-256; checkpoints use weights-only loading.
"""

import argparse
import hashlib
import importlib.util
import json
from pathlib import Path
import urllib.request

import numpy as np
import onnx
import onnxruntime as ort
import torch
from PIL import Image, ImageDraw, __version__ as pillow_version


SOURCES = {
    "UVDoc-LICENSE.txt": (
        "https://raw.githubusercontent.com/tanguymagne/UVDoc/4c9b82b537057aff2526e6dd118a847cdd072e82/LICENSE",
        "cf80bdbde76b47756c7fa62b56854ae54902979016bb08a6ba2edbebf93e9b90",
    ),
    "SyntheticDoc-LICENSE.txt": (
        "https://raw.githubusercontent.com/tanguymagne/SyntheticDoc/aabd9cf05f479781df432e3bd7e940f3e6716dbd/LICENSE",
        "28672dccef796c564b4d94ce410270408fe8bee0ee73bcbfc0a549fa0fb2ca66",
    ),
    "uvdoc_model.py": (
        "https://raw.githubusercontent.com/tanguymagne/UVDoc/4c9b82b537057aff2526e6dd118a847cdd072e82/model.py",
        "320f460edc54cf830dfdd9788dfbc1b96a836429b64ed6078a30582be6f59e42",
    ),
    "uvdoc.pkl": (
        "https://raw.githubusercontent.com/tanguymagne/UVDoc/4c9b82b537057aff2526e6dd118a847cdd072e82/model/best_model.pkl",
        "7e90861b8a516eb4bc51f84bd889cb77275743d2d1d3ca8091951ec9f2b7da23",
    ),
    "syntheticdoc_model.py": (
        "https://raw.githubusercontent.com/tanguymagne/SyntheticDoc/aabd9cf05f479781df432e3bd7e940f3e6716dbd/training/model.py",
        "8914ad8b16b04044ba1da29fc87ef8f798f550f02dda55171201cad6d176a6db",
    ),
    "syntheticdoc.ckpt": (
        "https://igl.ethz.ch/projects/SyntheticDoc/original_31x45.ckpt",
        "557af82579157c000f3cbd27c79d02b2a7cfea177849dbb04b9499ff88c81b1f",
    ),
}


def checked_source(cache, name):
    url, expected = SOURCES[name]
    path = cache / name
    if not path.exists():
        urllib.request.urlretrieve(url, path)
    actual = hashlib.sha256(path.read_bytes()).hexdigest()
    if actual != expected:
        raise ValueError(f"SHA-256 mismatch for {name}: {actual}")
    return path


def load_reference(cache, name):
    source = checked_source(cache, f"{name}_model.py")
    spec = importlib.util.spec_from_file_location(f"{name}_reference", source)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    if name == "uvdoc":
        model = module.UVDocnet(num_filter=32, kernel_size=5)
        state = torch.load(checked_source(cache, "uvdoc.pkl"), map_location="cpu", weights_only=True)
        model.load_state_dict(state["model_state"])
        size = (712, 488)
    else:
        state = torch.load(checked_source(cache, "syntheticdoc.ckpt"), map_location="cpu", weights_only=True)
        config = state["hyper_parameters"]
        size = tuple(config["data"]["image_size"])
        model = module.UVDocNet(
            num_filter=config["model"]["num_filter"],
            kernel_size=config["model"]["kernel_size"],
            bm_out_size=tuple(config["data"]["bm_out_size"]),
            output_size=size,
        )
        model.load_state_dict({k.removeprefix("model."): v for k, v in state["state_dict"].items()})
    return model.eval(), size


class Export(torch.nn.Module):
    def __init__(self, model, name):
        super().__init__()
        self.model, self.name = model, name

    def forward(self, image):
        result = self.model(image)
        if self.name == "uvdoc":
            return result[0]  # The 3D training head is unnecessary for unwarping.
        return result["bm_map"], result["shadow_map"]


def write_reference_fixture(model, name, size, output_dir, resized=False):
    """Self-authored document; expected pixels use upstream PyTorch semantics."""
    height, width = size
    if resized:
        height, width = height * 3 // 2, width * 3 // 2
    image = Image.new("RGB", (width, height), (55, 62, 70))
    draw = ImageDraw.Draw(image)
    draw.rounded_rectangle((35, 25, width - 25, height - 30), 8, fill=(235, 223, 202))
    draw.text((65, 65), "DOCUMENT UNWARP / REFERENCE", fill=(18, 28, 38), font_size=18)
    for row in range(18):
        y = 115 + row * 27
        draw.text((65, y), f"{row+1:02}  The quick brown fox / 0123456789", fill=(35, 45, 55), font_size=14)
    draw.rectangle((65, height - 110, width - 55, height - 70), outline=(40, 80, 135), width=3)
    array = np.asarray(image).astype(np.float32) / 255
    source = torch.from_numpy(array.transpose(2, 0, 1).copy()).unsqueeze(0)
    if name == "uvdoc":
        inp = torch.nn.functional.interpolate(source, size=size, mode="bilinear", align_corners=False)
    else:
        resized_image = image.resize((size[1], size[0]), Image.Resampling.BILINEAR)
        inp = torch.from_numpy(np.asarray(resized_image).astype(np.float32).transpose(2, 0, 1).copy()).unsqueeze(0) / 255 * 2 - 1
    prefix = name + ("-resized" if resized else "")
    with torch.inference_mode():
        predictions = model(inp)
        grid = predictions[0] if name == "uvdoc" else predictions["bm_map"]
        grid = torch.nn.functional.interpolate(grid, size=(height, width), mode="bilinear", align_corners=True)
        coordinates = grid.permute(0, 2, 3, 1)
        for illumination in ([False] if name == "uvdoc" else [False, True]):
            pixels = source
            if illumination:
                shadow = predictions["shadow_map"] * 0.5 + 0.5
                shadow = torch.nn.functional.interpolate(shadow, size=(height, width), mode="bilinear", align_corners=False)
                strength = min(1.0, 0.4 / max(source.mean().item(), 1e-6))
                shadow = (shadow * strength + 1 - strength).clamp(min=0.1)
                pixels = (pixels / shadow).clamp(0, 1)
            if name == "syntheticdoc":
                pixels = pixels * 2 - 1
            result = torch.nn.functional.grid_sample(pixels, coordinates, align_corners=True)
            if name == "syntheticdoc":
                result = result * 0.5 + 0.5
            result = (result[0].permute(1, 2, 0).numpy() * 255).clip(0, 255).astype(np.uint8)
            suffix = "-illuminated" if illumination else ""
            Image.fromarray(result).save(output_dir / f"{prefix}{suffix}-expected.png")
    image.save(output_dir / f"{prefix}-input.png")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cache-dir", type=Path, required=True)
    parser.add_argument("--output-dir", type=Path, default=Path("crates/clip-studios/assets/document-unwarp"))
    parser.add_argument("--fixture-dir", type=Path, help="Regenerate self-authored PyTorch reference fixtures")
    args = parser.parse_args()
    args.cache_dir.mkdir(parents=True, exist_ok=True)
    args.output_dir.mkdir(parents=True, exist_ok=True)
    torch.set_num_threads(4)
    torch.manual_seed(317)
    manifest = {
        "sources": SOURCES, "models": {}, "torch": torch.__version__,
        "onnx": onnx.__version__, "onnxruntime": ort.__version__,
        "numpy": np.__version__, "pillow": pillow_version,
    }
    for name in ("uvdoc", "syntheticdoc"):
        model, size = load_reference(args.cache_dir, name)
        wrapper = Export(model, name).eval()
        input_image = torch.rand(1, 3, *size)
        if name == "syntheticdoc":
            input_image = input_image * 2 - 1
        path = args.output_dir / f"{name}.onnx"
        names = ["grid"] if name == "uvdoc" else ["grid", "shadow"]
        with torch.inference_mode():
            torch.onnx.export(wrapper, (input_image,), path, input_names=["image"],
                              output_names=names, opset_version=17, dynamo=False,
                              external_data=False)
            expected = wrapper(input_image)
        license_name = "UVDoc-LICENSE.txt" if name == "uvdoc" else "SyntheticDoc-LICENSE.txt"
        license_text = checked_source(args.cache_dir, license_name).read_text()
        (args.output_dir / license_name).write_text(license_text)
        graph = onnx.load(str(path))
        onnx.helper.set_model_props(graph, {
            "upstream_license": license_text,
            "upstream_source": SOURCES[f"{name}_model.py"][0],
            "checkpoint_sha256": SOURCES["uvdoc.pkl" if name == "uvdoc" else "syntheticdoc.ckpt"][1],
        })
        onnx.save(graph, str(path))
        expected = [expected] if name == "uvdoc" else list(expected)
        onnx.checker.check_model(str(path))
        options = ort.SessionOptions()
        options.intra_op_num_threads = 4
        session = ort.InferenceSession(str(path), sess_options=options, providers=["CPUExecutionProvider"])
        actual = session.run(None, {"image": input_image.numpy()})
        errors = {}
        for output, reference, prediction in zip(names, expected, actual):
            np.testing.assert_allclose(prediction, reference.numpy(), atol=3e-5, rtol=3e-4)
            errors[output] = float(np.max(np.abs(prediction - reference.numpy())))
        manifest["models"][name] = {
            "input_height_width": size,
            "output_shapes": {key: list(value.shape) for key, value in zip(names, actual)},
            "sha256": hashlib.sha256(path.read_bytes()).hexdigest(),
            "bytes": path.stat().st_size,
            "maximum_absolute_export_error": errors,
        }
        print(name, manifest["models"][name], flush=True)
        if args.fixture_dir:
            args.fixture_dir.mkdir(parents=True, exist_ok=True)
            write_reference_fixture(model, name, size, args.fixture_dir)
            write_reference_fixture(model, name, size, args.fixture_dir, resized=True)
    (args.output_dir / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")


if __name__ == "__main__":
    main()
