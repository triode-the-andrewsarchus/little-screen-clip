#!/usr/bin/env python3
"""Passive V4L2/media device previews on private Weston/Niri."""
from pathlib import Path
import json
import subprocess
from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance


def fixtures(root, binary):
    video = next(iter(sorted(Path('/sys/class/video4linux').iterdir())))
    (root / 'a-video').symlink_to(video, target_is_directory=True)
    observation = root / '.video-device-observation.json'
    subprocess.run([str(binary.resolve()), '--internal-system-inspector', str(video), str(observation)], check=True, timeout=3)
    document = json.loads(observation.read_text())
    media = next(Path(node['path']) for node in document['topology']['nodes'] if 'Media controller' in node['label'])
    (root / 'b-media').symlink_to(media, target_is_directory=True)
    (root / 'notes.txt').write_text('Ordinary file beside passive media hardware observations.\n')


def exercise(session, root):
    session.observe('01-media-strips', ['a-video', 'b-media', 'V4L2'])
    session.click(848, 201)
    session.observe('02-mixed-folder', ['a-video', 'b-media', 'notes.txt'])
    x, y = session.word('02-mixed-folder', 'a-video')
    session.click(x + 32, y + 55)
    session.observe('03-video-inspector', ['V4L2 video interface', 'Video device name', 'Media driver'])
    session.key('Escape')
    session.observe('04-folder-return', ['a-video', 'b-media'])
    x, y = session.word('04-folder-return', 'b-media')
    session.click(x + 32, y + 55)
    session.observe('05-media-inspector', ['Media controller', 'Media controller model', 'Media driver'])
    session.key('Escape')
    session.observe('06-final-folder', ['a-video', 'b-media', 'notes.txt'])


def main():
    args = application_arguments(__doc__, Path('target/release/clip-file-manager'))
    output, root = media_fixture_directory(args)
    fixtures(root, args.binary)
    return run_application_acceptance(args, output, root, exercise, 'passive V4L2 and media-controller previews')


if __name__ == '__main__':
    raise SystemExit(main())
