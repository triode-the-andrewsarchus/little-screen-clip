#!/usr/bin/env python3
"""Exercise native Kontakt version dispatch with owned binary/XML payloads."""
import json
from pathlib import Path
import struct
import subprocess
import unittest
import zlib

ROOT = Path(__file__).resolve().parents[1]
HELPER = ROOT / 'target/release/nki-helper/little-clip-nki'
FIXTURES = ROOT / 'crates/little-clip-nki/tests/fixtures'


def chunk(kind, data):
    return struct.pack('<HI', kind, len(data)) + data


def structure(public, children=b'', version=0x80):
    return (b'\1' + struct.pack('<HI', version, 0)
            + struct.pack('<I', len(public)) + public
            + struct.pack('<I', len(children)) + children)


def header(version, compressed_length):
    data = bytearray(182 if version == 0x110 else 170)
    data[:4] = struct.pack('>I', 0x1290a87f)
    struct.pack_into('<IHIH', data, 4, compressed_length, version,
                     0x1a6337ea if version == 0x110 else 0x722a013e, 0)
    return data


def binary_payload():
    # One real-framed zone; its sample is deliberately unresolved, so this tests
    # mapping inspection independently from loading a sample recording.
    zone = struct.pack('<IIIHHHHHHHHHfffIIIIBI', 0, 100, 0, 0, 127, 48, 72,
                       0, 0, 0, 0, 60, 1, 0, 0, 0, 16, 22050, 1, 0, 100)
    zones = chunk(0x34, struct.pack('<II', 1, 0) + structure(zone, version=0x98))
    name = 'Owned 4.2 map'
    text = struct.pack('<I', len(name)) + name.encode('utf-16le')
    return chunk(0x28, structure(text, zones))


class NativeKontakt(unittest.TestCase):
    def invoke(self, data, success=True):
        result = subprocess.run([str(HELPER)], input=data, capture_output=True, timeout=20)
        if success:
            self.assertEqual(result.returncode, 0, result.stderr.decode())
            return json.loads(result.stdout)
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse(result.stdout)

    def test_kontakt_two_xml_and_monolith_rejection(self):
        legacy = (FIXTURES / 'legacy.nki').read_bytes()
        xml = zlib.decompress(legacy[36:]).replace(b'NiSS_', b'K2_')
        compressed = zlib.compress(xml)
        data = header(0x100, len(compressed)) + compressed
        self.assertEqual(self.invoke(data)[0]['regions'][1]['velocity'], [64, 127])
        struct.pack_into('<I', data, 42, 1)
        self.invoke(data, success=False)

    def test_four_two_fastlz_mapping_and_checksum(self):
        preset = binary_payload()
        # FastLZ literal packets, independent of the helper's decompressor.
        compressed = b''.join(bytes([len(preset[i:i+32])-1]) + preset[i:i+32]
                              for i in range(0, len(preset), 32))
        data = (header(0x110, len(compressed))
                + struct.pack('<II', zlib.crc32(compressed), len(preset))
                + bytes(32) + compressed)
        region = self.invoke(data)[0]['regions'][0]
        self.assertEqual(region['key'], [48, 72])
        self.assertEqual(region['root'], 60)
        self.assertEqual(region['end'], 100)
        data[-1] ^= 1
        self.invoke(data, success=False)

    def test_modern_reference_mapping(self):
        instrument = self.invoke((FIXTURES / 'kontakt680.nki').read_bytes())[0]
        self.assertEqual(instrument['name'], 'FM8 Jam')
        self.assertEqual(len(instrument['regions']), 61)
        self.assertEqual(instrument['regions'][0]['root'], 71)


if __name__ == '__main__':
    unittest.main()
