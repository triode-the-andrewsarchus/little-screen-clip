#!/usr/bin/env python3
"""Build/package the separate LGPL-2.1-only CDP8 DSP helper with replaceable source."""
import argparse
import json
import os
import shutil
import subprocess
from pathlib import Path

from helper_package import package_directory

CDP8_REPO = 'https://github.com/ComposersDesktop/CDP8.git'
CDP8_REV = '28bc42c72c1a7cb0fab933acd1c433be958a787b'


def ensure_cdp8_binaries(target: Path, cdp8_dir: Path | None, fetch: bool) -> Path | None:
    if cdp8_dir is not None:
        release_dir = cdp8_dir / 'NewRelease'
        return release_dir if release_dir.is_dir() else cdp8_dir

    tools_dir = target / 'tools/cdp8'
    release_dir = tools_dir / 'NewRelease'
    if release_dir.is_dir() and any(release_dir.iterdir()):
        return release_dir

    if not fetch:
        return None

    tools_dir.parent.mkdir(parents=True, exist_ok=True)
    if not (tools_dir / '.git').is_dir():
        subprocess.run(['git', 'clone', '--no-checkout', CDP8_REPO, str(tools_dir)], check=True)
    subprocess.run(['git', '-C', str(tools_dir), 'checkout', CDP8_REV], check=True)
    build_dir = tools_dir / 'build'
    subprocess.run([
        'cmake', '-S', str(tools_dir), '-B', str(build_dir),
        '-DCMAKE_POLICY_VERSION_MINIMUM=3.5',
        '-DCMAKE_BUILD_TYPE=RelWithDebInfo',
    ], check=True)
    subprocess.run(['cmake', '--build', str(build_dir), '-j', str(os.cpu_count() or 4)], check=True)
    return release_dir if release_dir.is_dir() else None


def build(output=None, skip_build=False, cdp8_dir=None, fetch_cdp8=False):
    repo = Path(__file__).resolve().parents[1]
    source = repo / 'crates/little-clip-cdp'
    metadata = json.loads(subprocess.check_output(
        ['cargo', 'metadata', '--no-deps', '--format-version', '1'], cwd=repo, text=True))
    target = Path(metadata['target_directory'])
    if not skip_build:
        subprocess.run(['cargo', 'build', '--release', '-p', 'little-clip-cdp'], cwd=repo, check=True)
    binary = target / 'release/little-clip-cdp'
    output = output or binary.parent / 'cdp-helper'

    resolved_cdp8 = ensure_cdp8_binaries(
        target,
        cdp8_dir.resolve() if cdp8_dir else None,
        fetch_cdp8,
    )

    with package_directory(output) as stage:
        shutil.copy2(binary, stage / 'little-clip-cdp')
        shutil.copytree(source, stage / 'source')
        manifest = (source / 'Cargo.toml').read_text()
        manifest = manifest.replace('version.workspace = true', 'version = "0.1.0"')
        manifest = manifest.replace('edition.workspace = true', 'edition = "2024"')
        manifest = manifest.replace('serde.workspace = true', 'serde = { version = "1.0", features = ["derive"] }')
        manifest = manifest.replace('serde_json.workspace = true', 'serde_json = "1.0"')
        manifest += '\n[workspace]\n\n[profile.dev]\ndebug = "line-tables-only"\nincremental = false\n'
        (stage / 'source/Cargo.toml').write_text(manifest)
        shutil.copyfile(repo / 'Cargo.lock', stage / 'source/Cargo.lock')
        for name in ['README.md', 'COPYING.LESSER']:
            shutil.copyfile(source / name, stage / name)

        bin_count = 0
        if resolved_cdp8 is not None and resolved_cdp8.is_dir():
            bin_stage = stage / 'bin'
            bin_stage.mkdir(parents=True, exist_ok=True)
            for item in resolved_cdp8.iterdir():
                if item.is_file() and os.access(item, os.X_OK):
                    shutil.copy2(item, bin_stage / item.name)
                    bin_count += 1

        (stage / 'build.json').write_text(json.dumps({
            'implementation': 'cdp8-subprocess-helper',
            'license': 'LGPL-2.1-only',
            'cdp8_rev': CDP8_REV,
            'bundled_cdp8_binaries': bin_count,
        }, indent=2) + '\n')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output-dir', type=Path, help='Defaults to cdp-helper beside the release executable')
    parser.add_argument('--skip-build', action='store_true', help='Package a successful existing workspace build')
    parser.add_argument('--cdp8-dir', type=Path, help='Existing CDP8 checkout or NewRelease directory')
    parser.add_argument('--fetch-cdp8', action='store_true', help='Clone and build pinned CDP8 if not yet in target/tools/cdp8')
    args = parser.parse_args()
    build(
        args.output_dir.resolve() if args.output_dir else None,
        args.skip_build,
        args.cdp8_dir,
        args.fetch_cdp8,
    )


if __name__ == '__main__':
    main()
