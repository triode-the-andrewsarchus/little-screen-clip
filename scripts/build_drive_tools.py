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


def build(output, cache, source_archive=None, jobs=2):
    repo = Path(__file__).resolve().parents[1]
    patch = repo / 'scripts/drive_tools/fuse2fs-noreplace.patch'
    fingerprint = {'version': VERSION, 'source_sha256': SHA256,
                   'patch_sha256': hashlib.sha256(patch.read_bytes()).hexdigest()}
    stamp = output / 'build.json'
    required = ['fuse2fs', 'COPYING', 'source/' + ARCHIVE,
                'source/fuse2fs-noreplace.patch', 'source/rebuild.sh']
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
    source = cache / ('build-' + fingerprint['patch_sha256'][:16])
    if not source.exists():
        source.mkdir()
        with tarfile.open(archive) as bundle:
            bundle.extractall(source, filter='data')
        subprocess.run(['patch', '-p1', '-i', str(patch)], cwd=source / f'e2fsprogs-{VERSION}', check=True)
    tree = source / f'e2fsprogs-{VERSION}'
    if not (tree / 'Makefile').is_file():
        subprocess.run(['./configure', '--disable-nls', '--disable-elf-shlibs', '--enable-fuse2fs'], cwd=tree, check=True)
    subprocess.run(['make', '-j' + str(jobs), 'libs'], cwd=tree, check=True)
    subprocess.run(['make', '-j' + str(jobs), '-C', 'misc', 'fuse2fs'], cwd=tree, check=True)
    with package_directory(output) as stage:
        shutil.copy2(tree / 'misc/fuse2fs', stage / 'fuse2fs')
        source_bundle = stage / 'source'
        source_bundle.mkdir()
        shutil.copy2(archive, source_bundle / ARCHIVE)
        shutil.copy2(patch, source_bundle / patch.name)
        shutil.copy2(tree / 'NOTICE', stage / 'COPYING')
        (source_bundle / 'rebuild.sh').write_text(
            '#!/bin/sh\nset -eu\n'
            f'tar -xf {ARCHIVE}\ncd e2fsprogs-{VERSION}\n'
            'patch -p1 < ../fuse2fs-noreplace.patch\n'
            './configure --disable-nls --disable-elf-shlibs --enable-fuse2fs\n'
            'make -j2 libs\nmake -j2 -C misc fuse2fs\n')
        (stage / 'README.md').write_text(
            '# RAM filesystem helper\n\n'
            'This standalone GPL-2.0 e2fsprogs helper reads and writes bounded ext4 images. '
            'It is not linked into the application. The source archive, local atomic-NOREPLACE '
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
    args = parser.parse_args()
    if not 1 <= args.jobs <= 16:
        parser.error('--jobs must be between 1 and 16')
    output = args.output_dir or repo / 'target' / args.profile / 'drive-tools'
    build(output.resolve(), args.cache_dir.resolve(), args.source_archive, args.jobs)


if __name__ == '__main__':
    main()
