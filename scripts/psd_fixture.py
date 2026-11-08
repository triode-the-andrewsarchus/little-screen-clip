"""Generate owned PSD fixtures from the public Adobe file layout."""
from pathlib import Path
import struct
import zlib
import io


def packed(fmt, *values):
    return struct.pack(">" + fmt, *values)


def tagged(key, data):
    return b"8BIM" + key + packed("I", len(data)) + data + b"\0" * (len(data) % 2)


def layer(name, bounds, color, *, kind=None, hidden=False, opacity=255, mask=False):
    top, left, bottom, right = bounds
    count = max(0, bottom - top) * max(0, right - left)
    planes = [(channel, bytes([color[channel]]) * count) for channel in range(3)] if count else []
    if count:
        planes.append((-1, bytes([255]) * count))
    if mask:
        planes.append((-2, bytes([255]) * count))
    channels = [packed("H", 0) + data for _, data in planes]
    mask_info = packed("4iBBH", *bounds, 255, 0, 0) if mask else b""
    fallback = name.encode("latin1", errors="replace")[:255]
    pascal = bytes([len(fallback)]) + fallback
    pascal += b"\0" * (-len(pascal) % 4)
    unicode = name.encode("utf-16-be")
    extra = packed("I", len(mask_info)) + mask_info + packed("I", 0) + pascal
    extra += tagged(b"luni", packed("I", len(unicode) // 2) + unicode)
    if kind is not None:
        extra += tagged(b"lsct", packed("I", kind))
    record = packed("4iH", *bounds, len(planes))
    record += b"".join(packed("hI", channel, len(data)) for (channel, _), data in zip(planes, channels))
    record += b"8BIM" + (b"mul " if hidden else b"norm") + bytes([opacity, 0, 2 if hidden else 0, 0])
    return record + packed("I", len(extra)) + extra, b"".join(channels)


def layer_section(width, height, colors):
    entries = [
        layer("Artwork", (0, 0, 0, 0), (0, 0, 0), kind=1),
        layer("Top patch", (0, width // 2, height, width), colors[1], mask=True),
        layer("Hidden tint α", (-4, -8, height + 4, width + 8), (50, 200, 20), hidden=True, opacity=128),
        layer("Background", (0, 0, height, width), colors[0]),
        layer("Group end", (0, 0, 0, 0), (0, 0, 0), kind=3),
    ]
    info = packed("h", -len(entries)) + b"".join(record for record, _ in entries) + b"".join(data for _, data in entries)
    info += b"\0" * (len(info) % 2)
    return packed("I", len(info)) + info + packed("I", 0)


def sample_value(value, depth):
    if depth == 8:
        return bytes([value])
    if depth == 16:
        return packed("H", value * 257)
    if depth == 32:
        value /= 255
        linear = value / 12.92 if value <= .04045 else ((value + .055) / 1.055) ** 2.4
        return packed("f", linear)
    raise ValueError(depth)


def composite_rows(width, height, depth, channels, colors):
    rows = []
    for channel in range(channels):
        row = b"".join(sample_value(colors[x >= width // 2][channel] if channel < 3 else 255, depth) for x in range(width))
        rows.extend([row] * height)
    return rows


def predicted_row(row, width, depth):
    if depth == 16:
        values = struct.unpack(">" + "H" * width, row)
        return b"".join(packed("H", (value - (values[i-1] if i else 0)) & 65535) for i, value in enumerate(values))
    if depth == 32:
        row = bytes(row[x * 4 + byte] for byte in range(4) for x in range(width))
    return bytes((value - (row[i-1] if i else 0)) & 255 for i, value in enumerate(row))


def encoded_rows(rows, width, depth, compression):
    if compression == 0:
        return b"".join(rows)
    if compression == 1:
        encoded = [b"".join(bytes([len(row[i:i+128]) - 1]) + row[i:i+128] for i in range(0, len(row), 128)) for row in rows]
        return b"".join(packed("H", len(row)) for row in encoded) + b"".join(encoded)
    if compression in {2, 3}:
        if compression == 3:
            rows = [predicted_row(row, width, depth) for row in rows]
        return zlib.compress(b"".join(rows))
    raise ValueError(compression)


def thumbnail_resource(colors):
    from PIL import Image
    image = Image.new("RGB", (32, 16), colors[0])
    image.paste(colors[1], (16, 0, 32, 16))
    jpeg = io.BytesIO()
    image.save(jpeg, format="JPEG", quality=95, subsampling=0)
    data = jpeg.getvalue()
    resource = packed("6I2H", 1, 32, 16, 96, 1536, len(data), 24, 1) + data
    return b"8BIM" + packed("H", 1036) + b"\0\0" + packed("I", len(resource)) + resource + b"\0" * (len(resource) % 2)


def write(path, width=64, height=32, depth=8, compression=0, layered=False,
          colors=((30, 120, 210), (180, 60, 40)), thumbnail=False):
    """Write a two-panel RGB composite; optional group and hidden/masked layers."""
    channels = 4 if layered else 3
    header = b"8BPS" + packed("H", 1) + b"\0" * 6 + packed("HIIHH", channels, height, width, depth, 3)
    if layered:
        assert depth == 8
    layers = layer_section(width, height, colors) if layered else b""
    rows = composite_rows(width, height, depth, channels, colors)
    pixels = encoded_rows(rows, width, depth, compression)
    resources = thumbnail_resource(colors) if thumbnail else b""
    Path(path).write_bytes(header + packed("I", 0) + packed("I", len(resources)) + resources + packed("I", len(layers)) + layers + packed("H", compression) + pixels)


if __name__ == "__main__":
    root = Path(__file__).resolve().parent.parent / "crates/little-clip-core/tests/fixtures"
    write(root / "layers.psd", layered=True)
    write(root / "thumbnail.psd", layered=True, thumbnail=True)
    for depth, compression in [(8, 1), (8, 2), (8, 3), (16, 0), (16, 3), (32, 0), (32, 3)]:
        write(root / f"composite-{depth}-{compression}.psd", depth=depth, compression=compression)
