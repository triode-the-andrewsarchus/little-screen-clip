"""Provisioning/fallback checks; no downloads or GPU are required."""
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import signal
import sys
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
    def test_managed_setup_rejects_wrong_owner_or_shared_group_before_writes(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            arguments = ['setup.py', '--runtime', str(root/'runtime'), '--models', str(root/'models'), '--parent', '42']
            for parent, group in [(1, os.getpid()), (42, os.getpid()+1)]:
                with self.subTest(parent=parent, group=group), patch.object(sys, 'argv', arguments), \
                     patch.object(setup.os, 'getppid', return_value=parent), \
                     patch.object(setup.os, 'getpgrp', return_value=group), \
                     self.assertRaisesRegex(RuntimeError, 'private|own Linux process group'):
                    setup.main()
                self.assertFalse((root/'runtime').exists())
                self.assertFalse((root/'models').exists())

    def test_managed_setup_arms_group_cleanup_and_closes_parent_race(self):
        for parents, errno in [([42, 42], 0), ([42, 1], 0), ([42], -1)]:
            with self.subTest(parents=parents, prctl=errno), \
                 patch.object(setup.os, 'getppid', side_effect=parents), \
                 patch.object(setup.os, 'getpgrp', return_value=os.getpid()), \
                 patch.object(setup.os, 'killpg') as kill, \
                 patch.object(setup.signal, 'signal') as handler, \
                 patch.object(setup.ctypes, 'CDLL') as library:
                library.return_value.prctl.return_value = errno
                if errno:
                    with self.assertRaises(OSError):
                        setup.attach_parent(42)
                    kill.assert_not_called()
                else:
                    setup.attach_parent(42)
                    if parents[-1] == 1:
                        kill.assert_called_once_with(os.getpid(), signal.SIGKILL)
                    else:
                        kill.assert_not_called()
                        handler.call_args.args[1](signal.SIGTERM, None)
                        kill.assert_called_once_with(os.getpid(), signal.SIGKILL)
                handler.assert_called_once()
                library.return_value.prctl.assert_called_once_with(1, signal.SIGTERM, 0, 0, 0)

    def test_provisioned_worker_starts_outside_checkout_without_model_or_download(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            runtime = root / "runtime"
            with patch.object(sys, "argv", ["setup.py", "--runtime", str(runtime), "--models", str(root / "models")]), \
                 patch.object(setup, "build"), patch.object(setup, "download") as download, \
                 patch.object(setup, "emit"):
                setup.main()
            download.assert_called_once()
            for name in ["MODEL_NOTICES.md", "LICENSE.models.apache-2.0", "LICENSE.models.qwen", "LICENSE.llama.cpp", "LICENSE.recall"]:
                self.assertEqual((runtime / name).read_bytes(), (setup.HERE / name).read_bytes())
            process = subprocess.run([sys.executable, "-I", str(runtime / "worker.py"), "--help"],
                                     cwd=root, capture_output=True, text=True, timeout=10)
            self.assertEqual(process.returncode, 0, process.stderr)
            self.assertIn("--role", process.stdout)
            self.assertFalse((root / "models").exists())

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
                self.assertFalse((runtime / ".runtime-incomplete").exists())
                self.assertFalse((runtime / ".runtime-staging").exists())
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
            self.assertTrue((folder / ".runtime-incomplete").exists(), "failed handoff requires explicit repair")


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
