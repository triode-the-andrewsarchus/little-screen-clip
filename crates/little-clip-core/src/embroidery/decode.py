"""Read embroidery bytes and emit absolute needle positions and thread events."""
import io
import struct
import sys

extension, bundle, count_limit = sys.argv[1:]
count_limit = int(count_limit)
sys.path.insert(0, bundle)
import pystitch as stitch

READERS = {item["extension"]: item.get("reader") for item in stitch.supported_formats()}
RECORD = struct.Struct("<ii3BB")


def deny_external_resources(event, args):
    if event in {"open", "os.listdir", "os.scandir", "os.system"} or event.startswith(
        ("socket.", "subprocess.", "ctypes.", "os.exec", "os.spawn", "os.fork", "os.posix_spawn")
    ):
        raise PermissionError("External resources are unavailable in embroidery previews")


class BoundedPattern(stitch.EmbPattern):
    def add_stitch_absolute(self, command, x=0, y=0):
        if len(self.stitches) >= count_limit:
            raise ValueError("stitch budget exceeded")
        super().add_stitch_absolute(command, x, y)


def read_pattern(source):
    # Some readers accept incomplete data. Require the envelope where known.
    if extension == "dst" and (len(source) < 515 or b"LA:" not in source[:512]):
        raise ValueError("missing Tajima header")
    signatures = {"pes": b"#PES", "pec": b"#PEC0001", "vp3": b"%vsm%\x00"}
    if extension in signatures and not source.startswith(signatures[extension]):
        raise ValueError("missing embroidery signature")
    if extension == "jef":
        (offset,) = struct.unpack_from("<I", source)
        if not 0x74 <= offset < len(source):
            raise ValueError("missing Janome header")
    if extension == "exp" and (len(source) < 4 or len(source) % 2):
        raise ValueError("incomplete EXP commands")

    sys.addaudithook(deny_external_resources)
    return stitch.EmbPattern.read_embroidery(
        READERS[extension], io.BytesIO(source), pattern=BoundedPattern()
    )


def encode(pattern):
    result = bytearray()
    thread = 0
    sewn_in_block = False
    for x, y, flags in pattern.stitches:
        command = flags & stitch.COMMAND_MASK
        if command in (stitch.STITCH, stitch.SEQUIN_EJECT):
            sewn_in_block = True
            color = pattern.threadlist[thread] if 0 <= thread < len(pattern.threadlist) else None
            if color is None:
                result += RECORD.pack(round(x), round(y), 0, 0, 0, 1)
            else:
                rgb = color.color
                result += RECORD.pack(round(x), round(y), (rgb >> 16) & 255, (rgb >> 8) & 255, rgb & 255, 0)
        elif command == stitch.NEEDLE_SET:
            if extension == "tbf":
                # TBF selects a one-based needle in its embedded thread table.
                thread = ((flags >> 16) & 255) - 2
            elif sewn_in_block:
                thread += 1
            result += RECORD.pack(0, 0, 0, 0, 0, 3 if sewn_in_block else 2)
            sewn_in_block = False
        elif command in (stitch.COLOR_CHANGE, stitch.COLOR_BREAK):
            thread += 1
            sewn_in_block = False
            result += RECORD.pack(0, 0, 0, 0, 0, 3)
        elif command in (stitch.JUMP, stitch.TRIM):
            result += RECORD.pack(0, 0, 0, 0, 0, 2)
        elif command == stitch.END:
            break
    return result


try:
    pattern = read_pattern(sys.stdin.buffer.read())
    if len(pattern.stitches) > count_limit:
        raise ValueError("stitch budget exceeded")
    sys.stdout.buffer.write(encode(pattern))
except Exception as error:
    sys.exit(f"Could not decode {extension.upper()} stitches ({type(error).__name__})")
