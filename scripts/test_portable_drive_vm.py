"""Owned-disk admission checks without a hypervisor or host device I/O."""
import os
from pathlib import Path
import tempfile
import unittest

from portable_drive_vm import owned_state_disk


class OwnedStateDisk(unittest.TestCase):
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


if __name__ == '__main__':
    unittest.main()
