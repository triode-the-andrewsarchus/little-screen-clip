"""Prepared media descriptor ownership; no model or GPU is required."""
import base64
import ctypes
import fcntl
import io
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time
import unittest
from unittest.mock import patch
import wave

from test_runtime import load

worker = load('worker')
PNG = b'\x89PNG\r\n\x1a\n' + (13).to_bytes(4, 'big') + b'IHDR' + (32).to_bytes(4, 'big') * 2


class MediaTransport(unittest.TestCase):
    def test_native_reads_exact_sealed_bytes_and_every_outcome_closes_the_descriptor(self):
        audio = io.BytesIO()
        with wave.open(audio, 'wb') as output:
            output.setparams((1, 2, 16000, 0, 'NONE', 'not compressed'))
            output.writeframes(b'\0\0' * 16000)
        owner = self
        for op, data, budget in [('image', PNG, 140), ('image', PNG, 280), ('audio', audio.getvalue(), 280)]:
            for outcome in [0, 2, ValueError('native failure')]:
                class Library:
                    fd = None

                    def clip_search_embed_media(self, model, path, image, tokens, output, error, size):
                        owner.assertTrue(path.startswith(b'/proc/self/fd/'))
                        self.fd = int(Path(os.fsdecode(path)).name)
                        owner.assertEqual(Path(os.fsdecode(path)).read_bytes(), data)
                        owner.assertEqual(os.fstat(self.fd).st_nlink, 0)
                        owner.assertEqual(os.fstat(self.fd).st_mode & 0o777, 0o600)
                        owner.assertEqual(image, op == 'image')
                        owner.assertEqual(tokens, budget)
                        seals = fcntl.F_SEAL_WRITE | fcntl.F_SEAL_GROW | fcntl.F_SEAL_SHRINK | fcntl.F_SEAL_SEAL
                        owner.assertEqual(fcntl.fcntl(self.fd, fcntl.F_GET_SEALS) & seals, seals)
                        with owner.assertRaises(PermissionError):
                            os.write(self.fd, b'change')
                        if isinstance(outcome, Exception):
                            raise outcome
                        return outcome

                library = Library()
                request = {'op':op, 'data':base64.b64encode(data).decode(), 'budget':budget}
                with self.subTest(op=op, budget=budget, outcome=outcome):
                    if isinstance(outcome, Exception):
                        with self.assertRaisesRegex(ValueError, 'native failure'):
                            worker.media_embedding(library, None, request, ctypes.create_string_buffer(100))
                    else:
                        _, status = worker.media_embedding(library, None, request, ctypes.create_string_buffer(100))
                        self.assertEqual(status, outcome)
                    with self.assertRaises(OSError):
                        os.fstat(library.fd)

    def test_invalid_media_is_rejected_before_descriptor_allocation(self):
        requests = [{'op':'image','data':''}, {'op':'image','data':'!'},
                    {'op':'image','data':base64.b64encode(PNG).decode(),'budget':999},
                    {'op':'audio','data':base64.b64encode(b'not WAV').decode()},
                    {'op':'image','data':base64.b64encode(b'x' * (8 * 1024 * 1024 + 1)).decode()}]
        with patch.object(worker.os, 'memfd_create', side_effect=AssertionError('invalid input allocated an FD')):
            for request in requests:
                with self.subTest(request={k:v for k,v in request.items() if k != 'data'}), self.assertRaises((ValueError, wave.Error, EOFError)):
                    worker.media_embedding(None, None, request, ctypes.create_string_buffer(100))

    def test_sigkill_during_native_media_read_leaves_no_filesystem_artifact(self):
        code = '''import base64,ctypes,sys,time
from pathlib import Path
sys.path.insert(0,sys.argv[1])
from test_runtime import load
worker=load('worker')
class Library:
 def clip_search_embed_media(self,model,path,*args):
  Path(sys.argv[2]).write_bytes(path)
  time.sleep(20)
  return 0
worker.media_embedding(Library(),None,{'op':'image','data':sys.argv[3]},ctypes.create_string_buffer(100))
'''
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            scratch = root / 'scratch'
            scratch.mkdir()
            marker = root / 'held'
            child = subprocess.Popen([sys.executable, '-I', '-c', code, str(Path(__file__).parent),
                                      str(marker), base64.b64encode(PNG).decode()],
                                     env=dict(os.environ, TMPDIR=str(scratch)))
            try:
                deadline = time.monotonic() + 5
                while not marker.exists():
                    self.assertIsNone(child.poll(), 'helper exited before the controlled native call')
                    self.assertLess(time.monotonic(), deadline, 'helper did not reach native call')
                    time.sleep(.01)
                child.kill()
                child.wait(timeout=5)
                self.assertEqual(list(scratch.iterdir()), [], 'prepared media survived helper SIGKILL')
            finally:
                if child.poll() is None:
                    child.kill()
                    child.wait(timeout=5)


if __name__ == '__main__':
    unittest.main()
