#!/usr/bin/env python3
"""Build the isolated, quota-capable RAM filesystem helper with corresponding GPL source."""
import argparse
import hashlib
import json
from pathlib import Path
import shutil
import subprocess
import tarfile
import urllib.request

from helper_package import package_directory

VERSION = '1.47.4'
ARCHIVE = f'e2fsprogs-{VERSION}.tar.xz'
URL = f'https://www.kernel.org/pub/linux/kernel/people/tytso/e2fsprogs/v{VERSION}/{ARCHIVE}'
# Verified against the release's sha256sums.asc, not a locally generated trust value.
SHA256 = 'fd5bf388cbdbe006a3d3b318d983b2948382440acc85a87f1e7d108653e8db0b'


def build(output, cache, source_archive=None, jobs=2, test_io_gate=False):
    repo = Path(__file__).resolve().parents[1]
    patch = repo / 'scripts/drive_tools/fuse2fs-noreplace.patch'
    patches = [patch]
    fingerprint = {'version': VERSION, 'source_sha256': SHA256,
                   'patch_sha256': hashlib.sha256(patch.read_bytes()).hexdigest(),
                   'test_io_gate_sha256': None}
    if test_io_gate:
        if output.resolve() in {(repo / 'target' / profile / 'drive-tools').resolve() for profile in ['debug', 'release']}:
            raise ValueError('Test helper must not replace the normal packaged helper')
        patches.append(repo / 'scripts/drive_tools/fuse2fs-test-gate.patch')
        fingerprint['test_io_gate_sha256'] = hashlib.sha256(patches[-1].read_bytes()).hexdigest()
    stamp = output / 'build.json'
    required = ['fuse2fs', 'COPYING', 'source/' + ARCHIVE,
                'source/rebuild.sh'] + ['source/' + item.name for item in patches]
    if all((output / item).is_file() for item in required) and stamp.is_file():
        current = json.loads(stamp.read_text())
        if all(current.get(key) == value for key, value in fingerprint.items()):
            return
    subprocess.run(['pkg-config', '--exists', 'fuse3'], check=True)
    cache.mkdir(parents=True, exist_ok=True)
    archive = cache / ARCHIVE
    if source_archive:
        if source_archive.resolve() != archive.resolve():
            shutil.copyfile(source_archive, archive)
    elif not archive.exists():
        with urllib.request.urlopen(URL, timeout=60) as response, archive.open('wb') as target:
            shutil.copyfileobj(response, target, 256 * 1024)
    if hashlib.sha256(archive.read_bytes()).hexdigest() != SHA256:
        raise RuntimeError('Upstream e2fsprogs source checksum mismatch')
    tree = prepare_source(cache, archive, patches, fingerprint)
    subprocess.run(['make', '-j' + str(jobs), 'libs'], cwd=tree, check=True)
    subprocess.run(['make', '-j' + str(jobs), '-C', 'misc', 'fuse2fs'], cwd=tree, check=True)
    publish(output, tree, archive, patches, fingerprint)


def prepare_source(cache, archive, patches, fingerprint):
    source = cache / ('build-' + fingerprint['patch_sha256'][:16])
    if fingerprint['test_io_gate_sha256']:
        source = source.with_name(source.name + '-gate-' + fingerprint['test_io_gate_sha256'][:16])
    marker = source / '.patches-applied'
    if not marker.is_file() or marker.read_text() != json.dumps(fingerprint):
        with package_directory(source) as stage:
            with tarfile.open(archive) as bundle:
                bundle.extractall(stage, filter='data')
            for item in patches:
                subprocess.run(['patch', '-p1', '-i', str(item)], cwd=stage / f'e2fsprogs-{VERSION}', check=True)
            (stage / '.patches-applied').write_text(json.dumps(fingerprint))
    tree = source / f'e2fsprogs-{VERSION}'
    if not (tree / 'Makefile').is_file():
        subprocess.run(['./configure', '--disable-nls', '--disable-elf-shlibs', '--enable-fuse2fs'], cwd=tree, check=True)
    return tree


def publish(output, tree, archive, patches, fingerprint):
    with package_directory(output) as stage:
        shutil.copy2(tree / 'misc/fuse2fs', stage / 'fuse2fs')
        source_bundle = stage / 'source'
        source_bundle.mkdir()
        shutil.copy2(archive, source_bundle / ARCHIVE)
        for item in patches:
            shutil.copy2(item, source_bundle / item.name)
        shutil.copy2(tree / 'NOTICE', stage / 'COPYING')
        (source_bundle / 'rebuild.sh').write_text(
            '#!/bin/sh\nset -eu\n'
            f'tar -xf {ARCHIVE}\ncd e2fsprogs-{VERSION}\n'
            + ''.join(f'patch -p1 < ../{item.name}\n' for item in patches) +
            './configure --disable-nls --disable-elf-shlibs --enable-fuse2fs\n'
            'make -j2 libs\nmake -j2 -C misc fuse2fs\n')
        (stage / 'README.md').write_text(
            ('# TEST ONLY: gated RAM filesystem helper\n\n' if fingerprint['test_io_gate_sha256'] else '# RAM filesystem helper\n\n') +
            'This standalone GPL-2.0 e2fsprogs helper reads and writes bounded ext4 images. '
            'It is not linked into the application. The source archive, local atomic-NOREPLACE/directory-fsync '
            'patch and rebuild instructions are included under source/. Native libfuse3, libblkid '
            'and libuuid dependencies must be available on the host.\n\n'
            'Rebuild: cd source && sh rebuild.sh\n')
        (stage / 'build.json').write_text(json.dumps({**fingerprint, 'upstream': URL,
            'license': 'GPL-2.0', 'source_build': 'cd source && sh rebuild.sh'}, indent=2) + '\n')


def main():
    repo = Path(__file__).resolve().parents[1]
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--profile', choices=['debug', 'release'], default='release')
    parser.add_argument('--output-dir', type=Path)
    parser.add_argument('--cache-dir', type=Path, default=repo / 'target/drive-tools-source')
    parser.add_argument('--source-archive', type=Path)
    parser.add_argument('--jobs', type=int, default=2)
    parser.add_argument('--test-io-gate', action='store_true', help='Build a separate test-only helper with an inherited write gate')
    args = parser.parse_args()
    if not 1 <= args.jobs <= 16:
        parser.error('--jobs must be between 1 and 16')
    output = args.output_dir or repo / 'target' / args.profile / ('drive-tools-test' if args.test_io_gate else 'drive-tools')
    build(output.resolve(), args.cache_dir.resolve(), args.source_archive, args.jobs, args.test_io_gate)


if __name__ == '__main__':
    main()
