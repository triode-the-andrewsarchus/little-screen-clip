#!/usr/bin/env python3
"""Build the workspace and discard release artifacts outside its current graph.

Use Cargo's JSON messages, including fresh artifacts, rather than access times or
a disk quota. Only a successful, complete build may authorize cleanup. Callers
must serialize this with other Cargo commands, just like the explicit sweep.
"""
import json
import re
import shutil
import subprocess
import sys
from pathlib import Path


def artifact_hash(path):
    match = re.search(r"-([0-9a-f]{16})(?:\.|$)", path.name)
    return match[1] if match else None


def retain_path(path, retained):
    """Identify Cargo's hashed unit, including build-script output directories."""
    path = Path(path)
    if path.parent.name in ("deps", "examples"):
        profile, unit = path.parent.parent, path
    elif path.parent.parent.name == "build":
        profile, unit = path.parents[2], path.parent
    elif path.parent.name == "release":
        profile = path.parent
        # Cargo reports final executable paths rather than their hashed hard links.
        matches = [p for p in (profile / "deps").glob(path.stem.replace("-", "_") + "-*")
                   if p.is_file() and p.samefile(path)]
        if not matches:
            raise ValueError(f"No hashed Cargo artifact for {path}")
        for match in matches:
            retain_path(match, retained)
        return
    else:
        raise ValueError(f"Unrecognized Cargo output layout: {path}")
    digest = artifact_hash(unit)
    if profile.name != "release" or not digest or not (profile / ".fingerprint").is_dir():
        raise ValueError(f"Unrecognized Cargo unit: {path}")
    retained.setdefault(profile, set()).add(digest)


def prune_artifacts(retained):
    """Remove only hashed Cargo outputs; preserve binaries and bundled helpers."""
    # Validate every profile before deleting anything. Changed Cargo layouts fail
    # closed rather than treating an incomplete inventory as a cleanup plan.
    for profile, hashes in retained.items():
        known = {artifact_hash(p) for p in (profile / ".fingerprint").iterdir()}
        if not hashes or not hashes <= known:
            raise ValueError(f"Incomplete Cargo fingerprint inventory in {profile}")
    removed = 0
    for profile, hashes in retained.items():
        for directory in ("deps", "build", ".fingerprint", "examples"):
            root = profile / directory
            if not root.is_dir() or root.is_symlink():
                continue
            for path in root.iterdir():
                digest = artifact_hash(path)
                if digest is None or digest in hashes or path.is_symlink():
                    continue
                if path.is_dir():
                    shutil.rmtree(path)
                else:
                    path.unlink()
                removed += 1
    print(f"Release cache: removed {removed} obsolete artifact entries; kept current build graph.")


def build_workspace():
    retained = {}
    complete = False
    inventory_error = None
    with subprocess.Popen(
        ["cargo", "build", "--release", "--message-format=json-render-diagnostics"],
        stdout=subprocess.PIPE, text=True,
    ) as cargo:
        for line in cargo.stdout:
            try:
                message = json.loads(line)
            except json.JSONDecodeError:
                print(line, end="")
                inventory_error = "Unexpected non-JSON Cargo output"
                continue
            reason = message.get("reason")
            if reason == "compiler-message":
                print(message["message"].get("rendered", ""), end="", file=sys.stderr)
            elif reason == "build-finished":
                complete = message["success"]
            elif reason in ("compiler-artifact", "build-script-executed"):
                paths = (message["filenames"] if reason == "compiler-artifact"
                         else [message["out_dir"]])
                for path in paths:
                    try:
                        retain_path(path, retained)
                    except (OSError, ValueError) as error:
                        inventory_error = str(error)
        result = cargo.wait()
    if result:
        return result
    try:
        if not complete or not retained or inventory_error:
            raise ValueError(inventory_error or "Cargo did not report a complete artifact graph")
        prune_artifacts(retained)
    except (OSError, ValueError) as error:
        print(f"Skipping release cache cleanup: {error}", file=sys.stderr)
    return 0


if __name__ == "__main__":
    sys.exit(build_workspace())
