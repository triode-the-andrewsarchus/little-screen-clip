"""Resource observations must not confuse processes or replace unknowns with zero."""
import tempfile
from pathlib import Path
import unittest
from unittest.mock import patch
from types import SimpleNamespace
from collections import deque
from runtime_probe import Sampler, gpu_memory, resident_pages, requests
from worker import validate_media
import base64
import io
import wave


class RuntimeProbeTests(unittest.TestCase):
    def test_gpu_process_identity_and_unknown_memory_are_preserved(self):
        xml = '''<nvidia_smi_log><gpu><uuid>fixture</uuid><processes>
          <process_info><pid>12</pid><used_memory>500 MiB</used_memory><type>G</type></process_info>
          <process_info><pid>23</pid><used_memory>32 MiB</used_memory><type>C</type></process_info>
        </processes></gpu></nvidia_smi_log>'''
        self.assertEqual(gpu_memory(xml, 12)['gpu_bytes'], 500 * 1024 * 1024)
        self.assertEqual(gpu_memory(xml, 23)['gpu_bytes'], 32 * 1024 * 1024)
        absent = gpu_memory(xml, 99)
        self.assertIsNone(absent['gpu_bytes'])
        self.assertEqual(absent['gpu_status'], 'not-listed')
        unknown = gpu_memory(xml.replace('500 MiB', 'N/A'), 12)
        self.assertIsNone(unknown['gpu_bytes'])
        self.assertEqual(unknown['gpu_status'], 'unknown')

    def test_probe_media_reaches_production_limits_without_exceeding_them(self):
        cases = dict(requests(True))
        for name in ['image_1024', 'audio_30s']:
            request = cases[name]
            validate_media(request['op'], base64.b64decode(request['data']))
        with wave.open(io.BytesIO(base64.b64decode(cases['audio_30s']['data']))) as wav:
            self.assertEqual(wav.getnframes(), 480000)
        self.assertNotIn('audio_30s', dict(requests(False)))
        self.assertEqual(len(cases['near_limit_batch4']['inputs']), 4)

    def test_page_residency_probe_is_bounded_to_owned_file(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'pages'
            path.write_bytes(b'fixture' * 2048)
            resident, pages = resident_pages(path)
            self.assertGreater(pages, 0)
            self.assertGreaterEqual(resident, 0)
            self.assertEqual(resident, pages)  # The tiny newly written fixture is resident.

    def test_retiring_old_raw_samples_does_not_erase_known_peaks(self):
        sampler = Sampler()
        sampler.pid = 12
        sampler.records = deque(maxlen=1)
        xml = '<nvidia_smi_log><gpu><processes><process_info><pid>12</pid><used_memory>{} MiB</used_memory></process_info></processes></gpu></nvidia_smi_log>'
        with patch('runtime_probe.process_memory', side_effect=[{'VmHWM': 100}, {'VmHWM': 50}]), \
             patch('runtime_probe.subprocess.run', side_effect=[SimpleNamespace(stdout=xml.format(500)), SimpleNamespace(stdout=xml.format(32))]):
            sampler.capture()
            sampler.capture()
        self.assertEqual(sampler.total_samples, 2)
        self.assertEqual(len(sampler.records), 1)
        self.assertEqual(sampler.peaks, {'gpu_bytes': 500 * 1024 * 1024, 'VmHWM': 100})


if __name__ == '__main__':
    unittest.main()
