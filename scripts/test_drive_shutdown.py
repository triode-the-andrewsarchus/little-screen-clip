"""Owned shutdown fixtures: no running user daemon or attached media are touched."""
import os
from pathlib import Path
import subprocess
import tempfile
import time
import unittest

REPO = Path(__file__).resolve().parents[1]


class ShutdownTests(unittest.TestCase):
    def launcher(self, directory, busy):
        source = (REPO / 'launch.sh').read_text()
        start = source.index('function stop_daemon() {')
        end = source.index('\n}\n', start) + 3
        mock = '''
set -e
PID_FILE="$1/pid"
SOCKET_PATH="$1/socket"
DRIVE_STOP_TRACE="$1/trace"
YELLOW='' GREEN='' RED='' NC=''
is_systemd_run() { return 0; }
is_running() { [ "$drive_test_running" = 1 ]; }
get_pid() { echo 12345; }
kill() { echo "kill $*" >> "$DRIVE_STOP_TRACE"; }
pkill() { echo "pkill $*" >> "$DRIVE_STOP_TRACE"; }
sleep() { if [ "$drive_test_busy" = 0 ]; then drive_test_running=0; fi; }
drive_test_running=1
drive_test_busy="$2"
LITTLE_CLIP_STOP_TIMEOUT_SECONDS=1
'''
        return subprocess.run(['bash', '-s', '--', str(directory), str(int(busy))],
                              input=mock + source[start:end] + '\nstop_daemon\n',
                              text=True, capture_output=True)

    def test_busy_stop_preserves_pid_socket_and_never_sends_sigkill(self):
        with tempfile.TemporaryDirectory() as temp:
            directory = Path(temp)
            for name in ('pid', 'socket'):
                (directory / name).write_text('owned fixture')
            result = self.launcher(directory, True)
            self.assertEqual(result.returncode, 1, result.stderr)
            self.assertTrue((directory / 'pid').exists())
            self.assertTrue((directory / 'socket').exists())
            self.assertNotIn('-9', (directory / 'trace').read_text())

    def test_completed_stop_removes_only_owned_lifecycle_files(self):
        with tempfile.TemporaryDirectory() as temp:
            directory = Path(temp)
            for name in ('pid', 'socket'):
                (directory / name).write_text('owned fixture')
            result = self.launcher(directory, False)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertFalse((directory / 'pid').exists())
            self.assertFalse((directory / 'socket').exists())

    @unittest.skipUnless(os.environ.get('CLIP_DRIVE_NATIVE_TESTS') == '1',
                         'requires the test-support drive fixture, tmpfs and FUSE')
    def test_sigterm_flushes_open_ram_file_before_process_exit_and_restart(self):
        # Reuse the existing acceptance transport; this test never launches a GUI.
        from portable_drive_acceptance import completed, request, status
        binary = REPO / 'target/debug/examples/drive_fixture'
        with tempfile.TemporaryDirectory() as temp:
            directory = Path(temp)
            runtime, source, state = [directory / name for name in ('runtime', 'source', 'state')]
            runtime.mkdir(mode=0o700)
            source.mkdir()
            (source / 'original').write_bytes(b'portable fixture')
            env = os.environ | {'XDG_RUNTIME_DIR': str(runtime),
                                'LITTLE_CLIP_DRIVE_STATE_DIR': str(state)}
            process = None
            held = None

            def launch():
                (runtime / 'little-screen-clip.sock').unlink(missing_ok=True)
                child = subprocess.Popen([str(binary), str(state), str(source)], env=env,
                                         stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
                deadline = time.monotonic() + 15
                while time.monotonic() < deadline:
                    self.assertIsNone(child.poll(), 'Owned fixture exited during startup')
                    try:
                        return child, status(env)
                    except (OSError, ValueError):
                        time.sleep(.05)
                child.kill()
                child.wait()
                self.fail('Owned fixture did not publish IPC')

            try:
                process, initial = launch()
                drive = initial['drives'][0]['id']
                completed(env, request(env, {'CopyToRam': {'drive': drive}}))
                ram = status(env)['ram'][0]
                completed(env, request(env, {'Pause': {'pair': ram['id'], 'paused': True}}))
                path = Path(ram['path']) / 'held-across-sigterm'
                held = os.open(path, os.O_CREAT | os.O_WRONLY, 0o600)
                payload = b'acknowledged write without close or fsync'
                os.write(held, payload)
                self.assertFalse((source / path.name).exists())
                process.terminate()
                self.assertEqual(process.wait(timeout=15), 0)
                process, _ = launch()
                deadline = time.monotonic() + 15
                while not status(env)['ram'][0]['backing']['mounted']:
                    self.assertLess(time.monotonic(), deadline)
                    time.sleep(.05)
                self.assertEqual(path.read_bytes(), payload)
                self.assertFalse((source / path.name).exists())
                os.close(held)
                held = None
                completed(env, request(env, {'DiscardRam': {'session': ram['id'], 'confirmed': True}}))
            finally:
                if held is not None:
                    os.close(held)
                if process is not None and process.poll() is None:
                    process.terminate()
                    try:
                        process.wait(timeout=15)
                    except subprocess.TimeoutExpired:
                        process.kill()
                        process.wait()


if __name__ == '__main__':
    unittest.main()
