"""Runtime publication and staging ownership without compilers, models or downloads."""
from contextlib import contextmanager
import fcntl
from pathlib import Path
import subprocess
import sys
import tempfile
import time
import unittest
from unittest.mock import patch

from test_runtime import load

setup, worker = load('setup'), load('worker')


class SetupTransaction(unittest.TestCase):
    def setUp(self):
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        self.root = Path(directory.name)
        self.runtime = self.root / 'runtime'
        self.runtime.mkdir()
        self.prefix = self.root / 'prefix'
        (self.prefix / 'lib').mkdir(parents=True)
        (self.prefix / 'lib/libggml-cpu.so').write_bytes(b'fixture dependency')

    @contextmanager
    def build_fixture(self):
        def compile_shim(arguments):
            self.assertEqual(arguments[0], 'cc')
            Path(arguments[-1]).write_bytes(b'fixture shim')
        with patch.object(setup, 'build_backends', return_value=['cpu']), \
             patch.object(setup, 'build_prefix', return_value=self.prefix), \
             patch.object(setup, 'run', side_effect=compile_shim) as compiler, \
             patch.object(setup, 'emit'):
            yield compiler

    def start_worker(self):
        arguments = ['worker.py', '--runtime', str(self.runtime), '--models', str(self.root/'models')]
        with patch.object(sys, 'argv', arguments), patch.object(worker, 'attach_parent'), \
             patch.object(worker, 'serve') as serve:
            worker.main()
            serve.assert_called_once()

    @contextmanager
    def held_installer(self, body, *arguments):
        marker = self.root/'held'
        marker.unlink(missing_ok=True)
        header = "import fcntl,sys,time\nfrom pathlib import Path\nsys.path.insert(0,sys.argv[1])\nfrom test_runtime import load\nsetup=load('setup')\n"
        child = subprocess.Popen([sys.executable, '-I', '-c', header+body,
                                  str(Path(__file__).parent), *map(str,arguments), str(marker)],
                                 start_new_session=True)
        try:
            deadline = time.monotonic() + 5
            while not marker.exists():
                self.assertIsNone(child.poll(), 'installer exited before its controlled gate')
                self.assertLess(time.monotonic(), deadline, 'installer did not reach its gate')
                time.sleep(.01)
            yield child
        finally:
            if child.poll() is None:
                child.kill()
            child.wait(timeout=5)

    def test_repeated_killed_compilers_leave_only_one_reserved_attempt(self):
        with self.build_fixture():
            setup.build(self.runtime, None)
        before = (self.runtime/'runtime.json').read_bytes()
        body = r'''runtime,prefix,held=map(Path,sys.argv[2:])
setup.build_backends=lambda prebuilt:['cpu','cuda']
setup.build_prefix=lambda folder,backends:prefix
setup.emit=lambda *args,**kwargs:None
def hold(arguments):
 held.write_text(str(arguments[-1]))
 time.sleep(20)
setup.run=hold
with (runtime/'setup.lock').open('a') as lock:
 fcntl.flock(lock,fcntl.LOCK_EX|fcntl.LOCK_NB)
 setup.build(runtime,None)
'''
        attempts = []
        for _ in range(2):
            with self.held_installer(body, self.runtime, self.prefix):
                attempts.append((self.root/'held').read_text())
            staging = self.runtime/'.runtime-staging'
            self.assertTrue(staging.is_dir())
            self.assertEqual(len(list(staging.iterdir())), 1)
            self.assertEqual(list(self.runtime.glob('.runtime-*')), [staging])
            self.assertEqual((self.runtime/'runtime.json').read_bytes(), before)
            self.assertEqual((self.runtime/'lib/libclip-search.so').read_bytes(), b'fixture shim')
            self.start_worker()
        self.assertNotEqual(attempts[0], attempts[1], 'old compiler paths must not target a new attempt')
        with self.build_fixture() as compiler:
            setup.build(self.runtime, None)
            compiler.assert_not_called()
        self.assertFalse((self.runtime/'.runtime-staging').exists())

    def test_live_model_shared_lock_prevents_setup_cleanup_or_download(self):
        staging = self.runtime/'.runtime-staging'
        staging.mkdir()
        (staging/'keep').write_bytes(b'active output')
        def while_serving(_arguments):
            arguments = ['setup.py','--runtime',str(self.runtime),'--models',str(self.root/'models')]
            with patch.object(sys,'argv',arguments), patch.object(setup,'build') as build, \
                 patch.object(setup,'download') as download, self.assertRaises(BlockingIOError):
                setup.main()
            build.assert_not_called()
            download.assert_not_called()
            self.assertEqual((staging/'keep').read_bytes(), b'active output')
        arguments = ['worker.py','--runtime',str(self.runtime),'--models',str(self.root/'models')]
        with patch.object(sys,'argv',arguments), patch.object(worker,'attach_parent'), \
             patch.object(worker,'serve',side_effect=while_serving) as serve:
            worker.main()
            serve.assert_called_once()

    def test_reused_runtime_cleans_only_its_reserved_staging_namespace(self):
        with self.build_fixture():
            setup.build(self.runtime, None)
        staging = self.runtime / '.runtime-staging'
        (staging / 'old-attempt').mkdir(parents=True)
        (staging / 'old-attempt/payload').write_bytes(b'abandoned output')
        unrelated = self.runtime / '.runtime-user-data'
        unrelated.mkdir()
        (unrelated / 'keep').write_bytes(b'unrelated')
        before = (self.runtime / 'runtime.json').read_bytes()
        with self.build_fixture() as compiler:
            setup.build(self.runtime, None)
            compiler.assert_not_called()
        self.assertFalse(staging.exists(), 'fast-path reuse must retire abandoned build scratch')
        self.assertEqual((unrelated / 'keep').read_bytes(), b'unrelated')
        self.assertEqual((self.runtime / 'runtime.json').read_bytes(), before)
        self.start_worker()

    def test_incomplete_or_legacy_handoff_blocks_before_native_loading(self):
        for kind in ['marker', 'backup', 'dangling-marker']:
            with self.subTest(kind=kind):
                runtime = self.root / kind
                runtime.mkdir()
                pending = runtime / ('lib.previous' if kind == 'backup' else '.runtime-incomplete')
                if kind == 'backup':
                    pending.mkdir()
                elif kind == 'dangling-marker':
                    pending.symlink_to(self.root/'absent')
                else:
                    pending.write_text('interrupted')
                arguments = ['worker.py', '--runtime', str(runtime), '--models', str(self.root/'models')]
                with patch.object(sys, 'argv', arguments), patch.object(worker, 'attach_parent'), \
                     patch.object(worker, 'serve') as serve, self.assertRaisesRegex(RuntimeError, 'interrupted'):
                    worker.main()
                serve.assert_not_called()

    def test_handoff_marker_does_not_follow_a_symlink(self):
        target = self.root/'unrelated'
        target.write_bytes(b'keep unrelated bytes')
        (self.runtime/'.runtime-incomplete').symlink_to(target)
        with self.assertRaises(OSError):
            setup.install_runtime(self.runtime, self.root/'absent', {})
        self.assertEqual(target.read_bytes(), b'keep unrelated bytes')
        self.assertFalse((self.runtime/'lib').exists())

    def test_killed_library_manifest_handoff_requires_explicit_repair(self):
        with self.build_fixture():
            setup.build(self.runtime, None)
        before = (self.runtime / 'runtime.json').read_bytes()
        staged = self.root / 'candidate'
        (staged / 'lib').mkdir(parents=True)
        for name in ['libclip-search.so', 'libclip-answer.so']:
            (staged / 'lib' / name).write_bytes(b'uncommitted shim')
        body = r'''runtime,staged,held=map(Path,sys.argv[2:])
replace=Path.replace
def hold(path,target):
 if Path(target)==runtime/'runtime.json':
  held.write_text('new library, old manifest')
  time.sleep(20)
 return replace(path,target)
Path.replace=hold
with (runtime/'setup.lock').open('a') as lock:
 fcntl.flock(lock,fcntl.LOCK_EX|fcntl.LOCK_NB)
 setup.install_runtime(runtime,staged,{'interrupted':True})
'''
        with self.held_installer(body, self.runtime, staged):
            pass
        self.assertEqual((self.runtime/'lib/libclip-search.so').read_bytes(), b'uncommitted shim')
        self.assertEqual((self.runtime/'runtime.json').read_bytes(), before)
        with self.assertRaisesRegex(RuntimeError, 'interrupted'):
            self.start_worker()
        with self.build_fixture() as compiler:
            setup.build(self.runtime, None)
            self.assertEqual(compiler.call_count, 2, 'interruption must bypass manifest-only reuse')
        self.assertFalse((self.runtime/'lib.previous').exists())
        self.assertFalse((self.runtime/'.runtime-incomplete').exists())
        self.assertEqual((self.runtime/'lib/libclip-search.so').read_bytes(), b'fixture shim')
        self.start_worker()


if __name__ == '__main__':
    unittest.main()
