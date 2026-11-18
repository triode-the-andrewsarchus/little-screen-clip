"""Owned-disk admission checks without a hypervisor or host device I/O."""
import os
import contextlib
import json
from pathlib import Path
import tempfile
import unittest
import sys
from types import SimpleNamespace

from portable_drive_vm import owned_state_disk, attach_disks, execute_guest


class OwnedStateDisk(unittest.TestCase):
    def test_usb_command_uses_the_pin_and_private_socket_and_refuses_disk_aliases(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            path = root / 'owned.raw'
            with path.open('xb') as file:
                file.truncate(64 * 1024**2)
            args = SimpleNamespace(state_disk=None, removable_disk=path)
            with contextlib.ExitStack() as resources:
                command = []
                handles, usb = attach_disks(args, resources, command, root / 'log')
                original = os.fstat(handles[0])
                path.rename(root / 'retained')
                path.write_bytes(b'foreign name replacement')
                file_node = json.loads(command[command.index('-blockdev') + 1])
                self.assertEqual(file_node['filename'], f'/proc/self/fd/{handles[0]}')
                self.assertEqual(os.stat(root / 'retained').st_ino, original.st_ino)
                socket_directory = usb.socket_path.parent
                self.assertTrue(socket_directory.is_dir())
            self.assertFalse(socket_directory.exists())
            args.state_disk = args.removable_disk = root / 'retained'
            with contextlib.ExitStack() as resources, self.assertRaises(ValueError):
                attach_disks(args, resources, [], root / 'log')

    def test_writable_pin_keeps_the_original_after_name_replacement(self):
        with tempfile.TemporaryDirectory() as temp:
            path = Path(temp) / 'owned.raw'
            with path.open('xb') as file:
                file.truncate(64 * 1024**2)
            disk = owned_state_disk(path)
            try:
                retained = path.with_suffix('.retained')
                path.rename(retained)
                path.write_bytes(b'foreign replacement')
                os.pwrite(disk, b'owned', 0)
                with retained.open('rb') as file:
                    self.assertEqual(file.read(5), b'owned')
                self.assertEqual(path.read_bytes(), b'foreign replacement')
            finally:
                os.close(disk)

    def test_non_regular_small_symlinked_and_aliased_paths_are_refused(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            image = root / 'owned.raw'
            with image.open('xb') as file:
                file.truncate(64 * 1024**2)
            link = root / 'symlink'
            link.symlink_to(image)
            fifo = root / 'fifo'
            os.mkfifo(fifo)
            small = root / 'small'
            small.write_bytes(b'owned')
            for path in [link, fifo, root, small]:
                with self.subTest(path=path.name), self.assertRaises((ValueError, OSError)):
                    owned_state_disk(path)
            alias = root / 'hardlink'
            os.link(image, alias)
            with self.assertRaises(ValueError):
                owned_state_disk(image)


class GuestExecution(unittest.TestCase):
    def test_nonzero_exit_is_retained_even_if_stdout_claims_success(self):
        with tempfile.TemporaryDirectory() as temp:
            args = SimpleNamespace(timeout=2)
            command = [sys.executable, '-c', 'print("LITTLE_CLIP_VM_RESULT=0"); raise SystemExit(7)']
            code, expired = execute_guest(args, command, Path(temp) / 'log', (), None)
            self.assertEqual(code, 7)
            self.assertFalse(expired)

    def test_timeout_is_distinct_from_successful_process_retirement(self):
        with tempfile.TemporaryDirectory() as temp:
            args = SimpleNamespace(timeout=0.05)
            command = [sys.executable, '-c', 'import time; time.sleep(20)']
            code, expired = execute_guest(args, command, Path(temp) / 'log', (), None)
            self.assertIsNone(code)
            self.assertTrue(expired)


if __name__ == '__main__':
    unittest.main()
