"""Provisioning/fallback checks; no downloads or GPU are required."""
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch


def load(name):
    spec = importlib.util.spec_from_file_location(name, Path(__file__).with_name(name + ".py"))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


setup, worker = load("setup"), load("worker")


class Provisioning(unittest.TestCase):
    def test_backend_changes_rebuild_and_success_retires_old_runtime(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            runtime, prefix = root / "runtime", root / "prefix"
            runtime.mkdir()
            (prefix / "lib").mkdir(parents=True)
            for name in ("libggml-base.so", "libggml-cpu.so", "libggml-cuda.so", "libggml-vulkan.so"):
                (prefix / "lib" / name).write_bytes(b"library")

            def compile_shim(args):
                self.assertEqual(args[0], "cc")
                Path(args[-1]).write_bytes(b"shim")

            with patch.object(setup, "build_backends", return_value=["cpu"]) as backends, \
                 patch.object(setup, "build_prefix", return_value=prefix) as build, \
                 patch.object(setup, "run", side_effect=compile_shim), \
                 patch.object(setup, "emit"):
                setup.build(runtime, None)
                self.assertFalse((runtime / "lib/libggml-cuda.so").exists())
                setup.build(runtime, None)
                self.assertEqual(build.call_count, 1, "Unchanged provisioning must reuse its build")
                backends.return_value = ["cpu", "cuda", "vulkan"]
                setup.build(runtime, None)
                self.assertEqual(build.call_count, 2)
                self.assertTrue((runtime / "lib/libggml-cuda.so").is_file())
                self.assertTrue((runtime / "lib/libggml-vulkan.so").is_file())
                self.assertFalse((runtime / "lib.previous").exists())
                original = (runtime / "runtime.json").read_bytes()
                backends.return_value = ["cpu"]
                with patch.object(setup, "run", side_effect=RuntimeError("compiler failed")):
                    with self.assertRaisesRegex(RuntimeError, "compiler failed"):
                        setup.build(runtime, None)
                self.assertEqual((runtime / "runtime.json").read_bytes(), original)
                self.assertEqual((runtime / "lib/libclip-search.so").read_bytes(), b"shim")

    def test_installation_failure_restores_old_library_and_manifest(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            folder, staged = root / "runtime", root / "staged"
            (folder / "lib").mkdir(parents=True)
            (folder / "lib/old.so").write_bytes(b"old library")
            (folder / "runtime.json").write_text('{"old":true}')
            staged.mkdir()
            # A failed staging handoff must preserve the prior installation.
            with self.assertRaises(FileNotFoundError):
                setup.install_runtime(folder, staged, {"new": True})
            self.assertEqual((folder / "lib/old.so").read_bytes(), b"old library")
            self.assertEqual(json.loads((folder / "runtime.json").read_text()), {"old": True})
            self.assertFalse((folder / "lib.previous").exists())


class DeviceSelection(unittest.TestCase):
    def test_explicit_gpu_requests_cannot_silently_select_cpu(self):
        class Library:
            def clip_search_device_name(self, output, size):
                output.value = b"CPU"
        with self.assertRaisesRegex(RuntimeError, "cuda backend is unavailable"):
            worker.selected_device(Library(), "cuda")
        self.assertEqual(worker.selected_device(Library(), "cpu"), ("cpu", "CPU"))
        self.assertEqual(worker.selected_device(Library(), "auto"), ("cpu", "CPU"))

    def test_vulkan_is_reported_as_a_real_gpu_backend(self):
        class Library:
            def clip_search_device_name(self, output, size):
                output.value = b"Vulkan0 (test GPU)"
        self.assertEqual(worker.selected_device(Library(), "vulkan"), ("vulkan", "Vulkan0 (test GPU)"))


if __name__ == "__main__":
    unittest.main()
