#!/usr/bin/env python3
"""Extend an owned frozen corpus with checksum-pinned local Commons media.

Download the sources listed in retrieval_media_validation.json separately.
This builder never downloads, indexes, or invokes a model. Expectations are
fixed before inference. Output must be a new folder; binaries remain local.
"""
import argparse
import hashlib
import json
from pathlib import Path
import shutil
import subprocess
import tempfile
import wave

from quality_fixtures import copy_frozen, frozen_bytes


def video(source, target, seconds=None, *, loop=False, audio=False):
    arguments = ['-stream_loop', '-1'] if loop else []
    arguments += ['-i', str(source)]
    if seconds is not None:
        arguments += ['-t', str(seconds)]
    arguments += ['-vf', 'fps=2,scale=384:216:force_original_aspect_ratio=decrease,pad=384:216:(ow-iw)/2:(oh-ih)/2,setsar=1',
                  '-map', '0:v:0', '-c:v', 'libx264', '-bf', '0', '-g', '2', '-pix_fmt', 'yuv420p']
    arguments += ['-map', '0:a?', '-c:a', 'aac', '-ac', '1', '-ar', '16000'] if audio else ['-an']
    transcode(arguments, target)


def transcode(arguments, target):
    subprocess.run(['ffmpeg', '-v', 'error', '-nostdin', '-n', '-threads', '2',
                    '-filter_threads', '1', '-protocol_whitelist', 'file,pipe',
                    *arguments, '-map_metadata', '-1', str(target)],
                   check=True, timeout=120)


def composite(background, event, target, start, total, work):
    parts = [work / (target.stem + f'-{index}.mp4') for index in range(3)]
    for source, length, output in zip([background, event, background],
                                      [start, 10, total - start - 10], parts):
        video(source, output, length, loop=True)
    listing = work / (target.stem + '.concat')
    listing.write_text(''.join(f"file '{part.name}'\n" for part in parts))
    transcode(['-f', 'concat', '-safe', '1', '-i', str(listing), '-c', 'copy'], target)


def long_audio(fixture, work, target, event_name, start):
    background, event = work / (target.stem + '-rain.wav'), work / (target.stem + '-event.wav')
    for source, output, duration, gain in [(fixture / 'sound_001.wav', background, 100, 0.3),
                                            (fixture / event_name, event, 10, 1)]:
        transcode(['-stream_loop', '-1', '-i', str(source), '-t', str(duration),
                   '-af', f'volume={gain}', '-ac', '1', '-ar', '16000', '-c:a', 'pcm_s16le'], output)
    with wave.open(str(background), 'rb') as source, wave.open(str(event), 'rb') as replacement:
        assert source.getparams()[:3] == replacement.getparams()[:3] == (1, 2, 16000)
        payload = bytearray(source.readframes(source.getnframes()))
        event_bytes = replacement.readframes(replacement.getnframes())
    assert len(payload) == 100 * 32000 and len(event_bytes) == 10 * 32000
    payload[start * 32000:(start + 10) * 32000] = event_bytes
    with wave.open(str(target), 'wb') as output:
        output.setparams((1, 2, 16000, 0, 'NONE', 'not compressed'))
        output.writeframes(payload)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--base', type=Path, required=True)
    parser.add_argument('--sources', type=Path, required=True)
    parser.add_argument('--output-dir', type=Path, required=True)
    args = parser.parse_args()
    profile_path = Path(__file__).with_name('retrieval_media_validation.json')
    profile = json.loads(profile_path.read_text())
    manifest = json.loads((args.base / 'fixture-manifest.json').read_text())
    args.output_dir.mkdir(parents=True, exist_ok=False)
    fixture = args.output_dir / 'fixture'
    copy_frozen(args.base / 'fixture', fixture, manifest)
    with tempfile.TemporaryDirectory(dir=args.output_dir) as temporary:
        work = Path(temporary)
        for record in profile['sources']:
            (work / record['name']).write_bytes(frozen_bytes(args.sources / record['name'], record))
        train, waterfall, fireworks = [work / name for name in
            ['Steam_train_at_station.webm', 'Small_waterfall.webm', 'Fireworks_in_summer.webm']]
        for number, source in enumerate([train, waterfall, fireworks], 2):
            video(source, fixture / f'movie_{number:03}.mp4', audio=True)
        composite(train, waterfall, fixture / 'movie_005.mp4', 90, 120, work)
        composite(waterfall, fireworks, fixture / 'movie_006.mp4', 60, 120, work)
        video(train, fixture / 'movie_007.mp4', 180, loop=True)
        long_audio(fixture, work, fixture / 'sound_012.wav', 'sound_000.wav', 65)
        long_audio(fixture, work, fixture / 'sound_013.wav', 'sound_009.wav', 82)
    for name, recipe in profile['recipes'].items():
        path = fixture / name
        manifest['files'].append(dict(name=name, kind='video' if name.startswith('movie') else 'audio',
            bytes=path.stat().st_size, sha256=hashlib.file_digest(path.open('rb'), 'sha256').hexdigest(),
            provenance=dict(recipe=recipe, profile_sha256=hashlib.sha256(profile_path.read_bytes()).hexdigest())))
    manifest['positives'].extend(profile['positives'])
    manifest['purpose'] += ' Extended with three independent Commons clips and controlled long-event composites; original expectations remain unchanged.'
    manifest['media_validation'] = profile
    (args.output_dir / 'fixture-manifest.json').write_text(json.dumps(manifest, indent=2))
    shutil.copyfile(args.base / 'validation-manifest.json', args.output_dir / 'validation-manifest.json')
    print(json.dumps(dict(files=len(manifest['files']), positives=len(manifest['positives']),
                          manifest_sha256=hashlib.sha256((args.output_dir / 'fixture-manifest.json').read_bytes()).hexdigest())))


if __name__ == '__main__':
    main()
