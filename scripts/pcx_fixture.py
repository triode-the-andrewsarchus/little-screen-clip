"""Owned PCX fixtures: Pillow RGB/indexed files and explicit legacy bit planes."""
from pathlib import Path
import struct
from PIL import Image


def rle(data):
    out = bytearray()
    at = 0
    while at < len(data):
        count = 1
        while count < 63 and at + count < len(data) and data[at + count] == data[at]:
            count += 1
        if count > 1 or data[at] >= 192:
            out.append(192 + count)
        out.append(data[at])
        at += count
    return out


def legacy(bits, planes, gray=False):
    width, height = 17, 5
    line = ((width * bits + 7) // 8 + 1) // 2 * 2
    header = bytearray(128)
    header[:4] = bytes([10, 5, 1, bits])
    struct.pack_into('<6H', header, 4, 3, 7, 3 + width - 1, 7 + height - 1, 300, 300)
    if (bits, planes) == (1, 1):
        header[19:22] = bytes([255, 255, 255])
    elif (bits, planes) in [(4, 1), (1, 4)]:
        header[16:64] = bytes(channel for i in range(16) for channel in (17*i, 255-17*i, 13*i))
    header[65] = planes
    struct.pack_into('<2H', header, 66, line, 2 if gray else 1)
    pixels = bytearray()
    for y in range(height):
        for plane in range(planes):
            row = bytearray(line)
            for x in range(width):
                index = (x + y) % (1 << (planes if bits == 1 else bits))
                value = (x + y*width)*3 if gray else (index >> plane if bits == 1 else index)
                shift = 8 - bits - (x * bits % 8)
                row[x * bits // 8] |= (value & ((1 << bits) - 1)) << shift
            row[(width * bits + 7) // 8:] = bytes([204]) * (line - (width * bits + 7) // 8)
            pixels.extend(rle(row))
    return header + pixels


def write_rgb(path, width=64, height=32, colors=((30, 120, 210), (180, 60, 40)), indexed=False):
    image = Image.new('RGB', (width, height), colors[0])
    image.paste(colors[1], (width // 2, 0, width, height))
    if indexed:
        image = image.quantize(colors=2, dither=Image.Dither.NONE)
    image.save(path, format='PCX')


if __name__ == '__main__':
    root = Path(__file__).resolve().parent.parent / 'crates/little-clip-core/tests/fixtures'
    write_rgb(root / 'panels.pcx')
    write_rgb(root / 'indexed.pcx', indexed=True)
    ramp = Image.new('P', (16,16))
    ramp.putpalette([channel for i in range(256) for channel in (255-i, i, i*13 % 256)])
    ramp.putdata(range(256))
    ramp.save(root / 'palette-ramp.pcx', format='PCX')
    for name, bits, planes in [('mono', 1, 1), ('cga-packed', 2, 1), ('cga-planar', 1, 2), ('ega-rgb', 1, 3), ('ega-planar', 1, 4), ('indexed4', 4, 1), ('gray', 8, 1)]:
        (root / f'{name}.pcx').write_bytes(legacy(bits, planes, gray=name == 'gray'))
