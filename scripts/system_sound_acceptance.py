#!/usr/bin/env python3
"""Passive sound-card and PCM previews on private Weston/Niri."""
from pathlib import Path
from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance


def fixtures(root):
    sound = Path('/sys/class/sound')
    pcm = next(path for path in sorted(sound.iterdir()) if path.name.startswith('pcmC') and path.name.endswith('p'))
    card = sound / ('card' + pcm.name.split('C', 1)[1].split('D', 1)[0])
    (root / 'a-card').symlink_to(card, target_is_directory=True)
    (root / 'b-playback').symlink_to(pcm, target_is_directory=True)
    (root / 'notes.txt').write_text('Ordinary file beside passive ALSA observations.\n')


def exercise(session, root):
    session.observe('01-sound-strips', ['a-card', 'b-playback', 'ALSA'])
    session.click(848, 201)
    session.observe('02-mixed-folder', ['a-card', 'b-playback', 'notes.txt'])
    x, y = session.word('02-mixed-folder', 'a-card')
    session.click(x + 32, y + 55)
    session.observe('03-card-inspector', ['ALSA sound card', 'ALSA card ID'])
    session.key('Escape')
    session.observe('04-folder-return', ['a-card', 'b-playback'])
    x, y = session.word('04-folder-return', 'b-playback')
    session.click(x + 32, y + 55)
    session.observe('05-playback-inspector', ['ALSA pcm playback', 'ALSA interface'])
    session.key('Escape')
    session.observe('06-final-folder', ['a-card', 'b-playback', 'notes.txt'])


def main():
    args = application_arguments(__doc__, Path('target/release/clip-file-manager'))
    output, root = media_fixture_directory(args)
    fixtures(root)
    return run_application_acceptance(args, output, root, exercise, 'passive ALSA sound previews')


if __name__ == '__main__':
    raise SystemExit(main())
