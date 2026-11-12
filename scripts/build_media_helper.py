#!/usr/bin/env python3
"""Build/package the isolated media worker and its minimal rebuildable workspace."""
import argparse
import json
from pathlib import Path
import shutil
import subprocess
import tomllib

from helper_package import package_directory


def build(output=None, skip_build=False, profile='release'):
    repo = Path(__file__).resolve().parents[1]
    metadata = json.loads(subprocess.check_output(
        ['cargo', 'metadata', '--no-deps', '--format-version', '1'], cwd=repo, text=True))
    target = Path(metadata['target_directory'])
    if not skip_build:
        command = ['cargo', 'build', '-p', 'clip-media-worker']
        if profile == 'release':
            command.append('--release')
        subprocess.run(command, cwd=repo, check=True)
    binary = target / profile / 'clip-media-worker'
    output = output or binary.parent / 'media-helper'
    manifest = (repo / 'Cargo.toml').read_text()
    workspace = tomllib.loads(manifest)['workspace']
    crates = ['clip-media-worker', 'clip-media-protocol']
    with package_directory(output) as stage:
        shutil.copy2(binary, stage / 'clip-media-worker')
        source = stage / 'source'
        for name in crates:
            shutil.copytree(repo / 'crates' / name, source / 'crates' / name)
        # Executable tests share canonical fixtures owned by core, not core code.
        fixtures = Path('crates/little-clip-core/tests/fixtures/video')
        shutil.copytree(repo / fixtures, source / fixtures)
        lines = ['[workspace]', 'resolver = "2"',
                 'members = ' + json.dumps(['crates/' + name for name in crates]),
                 '\n[workspace.package]']
        lines += [f'{key} = {json.dumps(value)}' for key, value in workspace['package'].items()]
        lines += ['\n[workspace.dependencies]']
        lines += [line for line in manifest.splitlines() if line.startswith(('serde = ', 'serde_json = ', 'rustix = '))]
        lines += ['\n[profile.dev]', 'debug = "line-tables-only"', 'incremental = false']
        (source / 'Cargo.toml').write_text('\n'.join(lines) + '\n')
        shutil.copyfile(repo / 'Cargo.lock', source / 'Cargo.lock')
        # Cargo prunes irrelevant workspace entries while preserving locked
        # versions. No dependencies are downloaded and no nested target is built.
        # Prefer offline mode to reuse cached dependencies, but fall back to network
        # if locked cross-platform crates (e.g. non-host targets) have not yet been fetched.
        metadata_cmd = ['cargo', 'metadata', '--format-version', '1',
                        '--manifest-path', str(source / 'Cargo.toml')]
        try:
            subprocess.run(['cargo', 'metadata', '--offline', '--format-version', '1',
                            '--manifest-path', str(source / 'Cargo.toml')],
                           cwd=repo, check=True, stdout=subprocess.DEVNULL)
        except subprocess.CalledProcessError:
            subprocess.run(metadata_cmd, cwd=repo, check=True, stdout=subprocess.DEVNULL)
        vendor_cmd = [
            'cargo', 'vendor', '--locked', '--versioned-dirs',
            '--manifest-path', str(source / 'Cargo.toml'), str(source / 'vendor'),
        ]
        try:
            vendor_config = subprocess.check_output([
                'cargo', 'vendor', '--offline', '--locked', '--versioned-dirs',
                '--manifest-path', str(source / 'Cargo.toml'), str(source / 'vendor')],
                cwd=repo, text=True)
        except subprocess.CalledProcessError:
            vendor_config = subprocess.check_output(vendor_cmd, cwd=repo, text=True)
        config = source / '.cargo/config.toml'
        config.parent.mkdir()
        # Vendor emits an absolute staging path; keep the delivered bundle movable.
        config.write_text(vendor_config.replace(str(source / 'vendor'), 'vendor'))
        for relative in ['scripts/timelens-GPL-2.0.txt', 'docs/audio/media_backend_migration.md', 'docs/audio/embedded_video_playback.md']:
            destination = source / relative
            destination.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(repo / relative, destination)
        shutil.copyfile(repo / 'scripts/timelens-GPL-2.0.txt', stage / 'COPYING')
        readme = (repo / 'crates/clip-media-worker/README.md').read_text()
        readme = readme.replace('../../scripts/timelens-GPL-2.0.txt', 'COPYING')
        readme = readme.replace('../../docs/audio/media_backend_migration.md', 'source/docs/audio/media_backend_migration.md')
        (stage / 'README.md').write_text(readme)
        (stage / 'build.json').write_text(json.dumps({
            'implementation': 'rust-ffmpeg-next-gstreamer', 'license': 'GPL-2.0-or-later',
            'profile': profile, 'native_libraries': 'FFmpeg 9 and GStreamer 1.24+ shared libraries/plugins required',
            'source_build': 'cd source && cargo build --release --locked --offline -p clip-media-worker',
        }, indent=2) + '\n')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output-dir', type=Path)
    parser.add_argument('--profile', choices=['debug', 'release'], default='release')
    parser.add_argument('--skip-build', action='store_true')
    args = parser.parse_args()
    build(args.output_dir.resolve() if args.output_dir else None, args.skip_build, args.profile)


if __name__ == '__main__':
    main()
