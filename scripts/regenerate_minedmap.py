#!/usr/bin/env python3
"""Regenerate the vendored renderer from a pinned upstream commit and local patches."""

import argparse
from pathlib import Path
import re
import subprocess
import tarfile
import tempfile

ROOT = Path(__file__).resolve().parent.parent
PATCHES = ROOT / "scripts/minedmap"
VENDOR = ROOT / "vendor/minedmap"
UPSTREAM = "https://github.com/neocturne/MinedMap.git"

# Preserve upstream paths so the local patches can be reviewed against MinedMap.
SOURCE_PATHS = (
    "Cargo.toml",
    "LICENSE",
    "rustfmt.toml",
    "src/util.rs",
    "src/world/chunk.rs",
    "src/world/de.rs",
    "src/world/layer.rs",
    "src/world/mod.rs",
    "src/world/section.rs",
    "crates/nbt/Cargo.toml",
    "crates/nbt/src",
)

# The README is maintained locally. Cargo can create the others when working
# directly in the nested workspace.
LOCAL_PATHS = (".git", "README.md", "Cargo.lock", "target")


def extract_sources(upstream: Path, revision: str, snapshot: Path) -> None:
    archive = snapshot.parent / "upstream.tar"
    with archive.open("wb") as output:
        subprocess.run(
            ["git", "-C", upstream, "archive", revision, "--", *SOURCE_PATHS],
            stdout=output,
            check=True,
        )
    snapshot.mkdir()
    with tarfile.open(archive) as source:
        source.extractall(snapshot, filter="data")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--check", action="store_true", help="check that regeneration matches the vendored source"
    )
    parser.add_argument(
        "upstream_checkout", nargs="?", type=Path, help="local Git checkout containing the pinned commit"
    )
    args = parser.parse_args()

    revision = (PATCHES / "upstream-revision").read_text().strip()
    if re.fullmatch(r"[0-9a-f]{40}", revision) is None:
        parser.error("upstream-revision must contain a full Git commit ID")

    with tempfile.TemporaryDirectory(prefix="minedmap-vendor-") as temp:
        scratch = Path(temp)
        upstream = args.upstream_checkout
        if upstream is None:
            upstream = scratch / "upstream"
            subprocess.run(["git", "init", "-q", upstream], check=True)
            subprocess.run(
                ["git", "-C", upstream, "fetch", "-q", "--no-tags", "--depth=1", UPSTREAM, revision],
                check=True,
            )

        snapshot = scratch / "snapshot"
        extract_sources(upstream, revision, snapshot)
        subprocess.run(["git", "init", "-q", snapshot], check=True)
        for patch in sorted(PATCHES.glob("*.patch")):
            subprocess.run(["git", "-C", snapshot, "apply", "--whitespace=error", patch], check=True)

        if args.check:
            result = subprocess.run(
                ["diff", "-ru", *(f"--exclude={name}" for name in LOCAL_PATHS), VENDOR, snapshot]
            )
            if result.returncode:
                return result.returncode
            print(f"MinedMap vendor files match {revision} and the patch series.")
        else:
            subprocess.run(
                [
                    "rsync", "-a", "--no-times", "--checksum", "--delay-updates", "--delete-delay",
                    *(f"--exclude={name}" for name in LOCAL_PATHS),
                    f"{snapshot}/", f"{VENDOR}/",
                ],
                check=True,
            )
            print(f"Regenerated vendor/minedmap from {revision}.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
