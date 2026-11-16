"""Lost-reply/restart acceptance using only an owned test-support IPC host."""
from concurrent.futures import ThreadPoolExecutor
import json
import os
from pathlib import Path
import socket
import subprocess
import tempfile
import time
import unittest
import uuid

REPO = Path(__file__).resolve().parents[1]
HOST = REPO / 'target/debug/examples/drive_fixture'
CLI = REPO / 'target/debug/clipctl'


@unittest.skipUnless(HOST.is_file() and CLI.is_file(), 'build the test-support drive_fixture example and clipctl')
class AdmissionTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='drive-admission-')
        self.directory = Path(self.temp.name)
        self.runtime = self.directory / 'runtime'
        self.runtime.mkdir(mode=0o700)
        self.source = self.directory / 'source'
        self.source.mkdir()
        (self.source / 'original.txt').write_bytes(b'owned admission payload')
        self.state = self.directory / 'state'
        self.env = os.environ | {
            'XDG_RUNTIME_DIR': str(self.runtime),
            'LITTLE_CLIP_DRIVE_STATE_DIR': str(self.state),
        }
        self.process = None
        self.log = (self.directory / 'fixture.log').open('wb')
        self.addCleanup(self.cleanup)
        self.launch()

    def exchange(self, request):
        with socket.socket(socket.AF_UNIX) as stream:
            stream.settimeout(5)
            stream.connect(str(self.runtime / 'little-screen-clip.sock'))
            stream.sendall(json.dumps(request).encode() + b'\n')
            line = stream.makefile('rb').readline(4 * 1024 * 1024)
        return json.loads(line)

    def request(self, request):
        response = self.exchange(request)
        self.assertNotIn('Error', response, response)
        return response['Drives']

    def status(self):
        return self.request({'Drives': {'Status': {'offset': 0}}})['Status']

    def wait(self, predicate):
        deadline = time.monotonic() + 15
        while time.monotonic() < deadline:
            self.assertIsNone(self.process.poll(), 'Owned fixture exited unexpectedly')
            result = predicate()
            if result:
                return result
            time.sleep(.02)
        self.fail('Owned admission fixture did not reach its expected state')

    def launch(self):
        # This directory/socket was created by this fixture, never by the user daemon.
        (self.runtime / 'little-screen-clip.sock').unlink(missing_ok=True)
        self.process = subprocess.Popen([str(HOST), str(self.state), str(self.source)],
                                        env=self.env, stdout=self.log, stderr=subprocess.STDOUT)
        def ready():
            try:
                return self.status()
            except (OSError, ValueError):
                return None
        return self.wait(ready)

    def stop(self):
        if self.process is not None and self.process.poll() is None:
            self.process.terminate()
            self.assertEqual(self.process.wait(timeout=15), 0)

    def cleanup(self):
        try:
            self.stop()
        finally:
            if self.process is not None and self.process.poll() is None:
                self.process.kill()
                self.process.wait()
            self.log.close()
            self.temp.cleanup()

    def completed(self, operation):
        def result():
            return next((op for op in self.status()['operations']
                         if op['id'] == operation and op['finished'] is not None), None)
        result = self.wait(result)
        self.assertEqual(result['state'], 'complete', result)
        return result

    def test_lost_reply_concurrent_retries_and_restart_keep_one_backup(self):
        backups = self.directory / 'backups'
        selected = self.request({'Drives': {'BackupDirectory': {'path': str(backups)}}})
        self.completed(selected['Accepted'])
        initial_operations = len(self.status()['operations'])
        review = self.request({'Drives': {'PreviewBackup': {
            'drive': 'part:portable-test', 'kind': 'Files',
            'destination': str(backups),
        }}})['BackupPreview']
        key = str(uuid.uuid4())
        payload = {'StartBackup': {'token': review['token'], 'restore_mounted': False,
                                   'remember_destination': False}}
        command = {'SubmitDrive': {'request_id': key, 'request': payload}}
        with socket.socket(socket.AF_UNIX) as dropped:
            dropped.settimeout(5)
            dropped.connect(str(self.runtime / 'little-screen-clip.sock'))
            dropped.sendall(json.dumps(command).encode() + b'\n')
            # Lose the reply after sending; the owner must retain the operation.
        self.wait(lambda: any(op['id'] == key for op in self.status()['operations']))
        with ThreadPoolExecutor(max_workers=8) as clients:
            replies = list(clients.map(lambda _: self.request(command), range(12)))
        self.assertEqual(replies, [{'Accepted': key}] * 12)
        completed = self.completed(key)
        state = self.status()
        self.assertEqual(len(state['operations']), initial_operations + 1)
        self.assertEqual(len(state['backups']), 1)
        version = state['backups'][0]
        self.assertEqual((Path(version['path']) / 'original.txt').read_bytes(), b'owned admission payload')
        changed = {'SubmitDrive': {'request_id': key, 'request': {
            'StartBackup': dict(payload['StartBackup'], remember_destination=True),
        }}}
        self.assertIn('different parameters', self.exchange(changed)['Error']['error'])
        self.stop()
        self.launch()
        self.assertEqual(self.request(command), {'Accepted': key})
        cli = subprocess.run([str(CLI), 'drives', 'start-backup', review['token'],
                              '--request-id', key], env=self.env, capture_output=True,
                             text=True, timeout=10)
        self.assertEqual(cli.returncode, 0, cli.stderr)
        self.assertEqual(json.loads(cli.stdout), {'Accepted': key})
        restored = self.status()
        self.assertEqual(len(restored['operations']), initial_operations + 1)
        self.assertEqual([item['id'] for item in restored['backups']], [version['id']])
        retried = next(op for op in restored['operations'] if op['id'] == key)
        self.assertEqual(retried['finished'], completed['finished'])

    def test_cli_lost_reply_reports_the_identity_for_safe_retry(self):
        proxy_runtime = self.directory / 'proxy-runtime'
        proxy_runtime.mkdir(mode=0o700)
        destination = self.directory / 'cli-backups'
        with socket.socket(socket.AF_UNIX) as proxy:
            proxy.settimeout(5)
            proxy.bind(str(proxy_runtime / 'little-screen-clip.sock'))
            proxy.listen(1)

            def drop_reply():
                peer, _ = proxy.accept()
                with peer:
                    peer.settimeout(5)
                    message = peer.makefile('rb').readline(1024 * 1024)
                    with socket.socket(socket.AF_UNIX) as host:
                        host.settimeout(5)
                        host.connect(str(self.runtime / 'little-screen-clip.sock'))
                        host.sendall(message)
                        response = json.loads(host.makefile('rb').readline(4 * 1024 * 1024))
                        self.assertIn('Drives', response, response)
                    # The daemon has acknowledged; deliberately discard that reply.
                return json.loads(message)

            with ThreadPoolExecutor(max_workers=1) as relay:
                delivered = relay.submit(drop_reply)
                failed = subprocess.run([str(CLI), 'drives', 'backup-directory', str(destination)],
                                        env=self.env | {'XDG_RUNTIME_DIR': str(proxy_runtime)},
                                        capture_output=True, text=True, timeout=10)
                message = delivered.result(timeout=10)
        key = message['SubmitDrive']['request_id']
        self.assertNotEqual(failed.returncode, 0)
        self.assertIn(key, failed.stderr)
        self.assertIn('--request-id', failed.stderr)
        self.completed(key)
        retried = subprocess.run([str(CLI), 'drives', '--request-id', key,
                                  'backup-directory', str(destination)], env=self.env,
                                 capture_output=True, text=True, timeout=10)
        self.assertEqual(retried.returncode, 0, retried.stderr)
        self.assertEqual(json.loads(retried.stdout), {'Accepted': key})
        self.assertEqual(len(self.status()['operations']), 1)
        self.assertTrue(destination.is_dir())


if __name__ == '__main__':
    unittest.main()
