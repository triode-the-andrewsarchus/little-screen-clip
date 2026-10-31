"""Trace evidence must match the target descriptor, not another path or pipe."""
from pathlib import Path
import unittest
from file_manager_offline_acceptance import payload_reads


class PayloadTraceTests(unittest.TestCase):
    def test_reads_and_mappings_include_device_annotations_but_exclude_other_descriptors(self):
        target = Path('/owned/sample.wav')
        selected = [
            'read(3</owned/sample.wav>, ""..., 8192) = 17',
            'pread64(8</owned/sample.wav>, ""..., 4096, 0) = 17',
            'readv(3</owned/sample.wav<char 13:1>>, [{iov_base=...}], 1) = 42',
            'mmap(NULL, 4096, PROT_READ, MAP_PRIVATE, 9</owned/sample.wav<block 8:0>>, 0) = 0x1000',
        ]
        unrelated = [
            'read(4</owned/sample.wav.saved>, ""..., 16) = 0',
            'read(0<pipe:[100]>, ""..., 128) = 23',
            'read(1</owned/stdout.log>, ""..., 128) = 23',
            'openat(AT_FDCWD, "/owned/sample.wav", O_RDONLY) = 3</owned/sample.wav>',
            'mmap(NULL, 4096, PROT_READ, MAP_PRIVATE|MAP_ANONYMOUS, -1, 0) = 0x1000',
        ]
        self.assertEqual(payload_reads({'worker':'\n'.join(selected + unrelated)}, target), selected)


if __name__ == '__main__':
    unittest.main()
