#!/usr/bin/env python3
"""Trace bounded system helpers and assert absence of forbidden content reads.

Requires Linux strace and the existing /dev/null, /proc/uptime and loopback sysfs
interface. Does not open devices for content, change attributes or require root.
"""
import argparse
import json
from pathlib import Path
import re
import subprocess
import tempfile


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=Path("target/release/clip-file-manager"))
    parser.add_argument("--output-dir", type=Path, required=True)
    args = parser.parse_args()
    output = args.output_dir.resolve()
    output.mkdir(parents=True, exist_ok=True)
    records = []
    with tempfile.TemporaryDirectory(prefix="system-io-") as root:
        for index, path in enumerate(["/dev/null", "/proc/uptime", "/sys/class/net/lo/operstate"]):
            trace = output / f"{index}.trace"
            document = Path(root) / f"{index}.json"
            subprocess.run([
                "strace", "-f", "-yy", "-e", "trace=openat,read", "-o", str(trace),
                str(args.binary.resolve()), "--internal-system-inspector", path, str(document),
            ], check=True, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, timeout=3)
            lines = trace.read_text().splitlines()
            canonical = str(Path(path).resolve())
            reads = [line for line in lines if re.search(r"read\(\d+<" + re.escape(canonical) + r"(?:<[^>]+>)?>", line)]
            opens = [line for line in lines if "openat(" in line and canonical in line and "O_PATH" not in line]
            if path == "/sys/class/net/lo/operstate":
                assert reads, "Documented attribute was not observed by the allowed collector"
            else:
                assert not opens and not reads, f"Forbidden content access to {path}: {opens + reads}"
            records.append({"path": path, "content_reads": len(reads), "content_opens": len(opens), "trace": str(trace)})
    report = {"status": "passed", "observations": records}
    (output / "report.json").write_text(json.dumps(report, indent=2))
    print(json.dumps(report))


if __name__ == "__main__":
    main()
