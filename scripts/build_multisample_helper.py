#!/usr/bin/env python3
"""Build/package the separate native LGPL NKI reader with replaceable source."""
import argparse
import json
import shutil
import subprocess
from pathlib import Path

from helper_package import package_directory


def build(output=None, skip_build=False):
    repo = Path(__file__).resolve().parents[1]
    source = repo / 'crates/little-clip-nki'
    metadata = json.loads(subprocess.check_output(
        ['cargo', 'metadata', '--no-deps', '--format-version', '1'], cwd=repo, text=True))
    target = Path(metadata['target_directory'])
    if not skip_build:
        subprocess.run(['cargo', 'build', '--release', '-p', 'little-clip-nki'], cwd=repo, check=True)
    binary = target / 'release/little-clip-nki'
    output = output or binary.parent / 'nki-helper'
    with package_directory(output) as stage:
        shutil.copy2(binary, stage / 'little-clip-nki')
        shutil.copytree(source, stage / 'source')
        # Resolve workspace inheritance so the delivered source builds on its own.
        manifest = (source / 'Cargo.toml').read_text()
        manifest = manifest.replace('version.workspace = true', 'version = "0.1.0"')
        manifest = manifest.replace('edition.workspace = true', 'edition = "2024"')
        manifest = manifest.replace('serde_json.workspace = true', 'serde_json = "1.0"')
        manifest += '\n[workspace]\n\n[profile.dev]\ndebug = "line-tables-only"\nincremental = false\n'
        (stage / 'source/Cargo.toml').write_text(manifest)
        shutil.copyfile(repo / 'Cargo.lock', stage / 'source/Cargo.lock')
        for name in ['README.md', 'COPYING', 'COPYING.LESSER', 'THIRD_PARTY_NOTICES']:
            shutil.copyfile(source / name, stage / name)
        (stage / 'build.json').write_text(json.dumps({'implementation': 'native-rust', 'license': 'LGPL-3.0-only'}, indent=2) + '\n')



def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output-dir', type=Path, help='Defaults to nki-helper beside the release executable')
    parser.add_argument('--skip-build', action='store_true', help='Package a successful existing workspace build')
    args = parser.parse_args()
    build(args.output_dir.resolve() if args.output_dir else None, args.skip_build)


if __name__ == '__main__':
    main()
