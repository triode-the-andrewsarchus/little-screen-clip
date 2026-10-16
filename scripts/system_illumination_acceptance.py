#!/usr/bin/env python3
"""LED strips and attributes on private Weston/Niri without changing outputs."""
from pathlib import Path
from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance, private


def fixtures(root):
    led = next(device for device in sorted(Path('/sys/class/leds').iterdir())
               if (device / 'brightness').is_file() and (device / 'trigger').is_file())
    (root / 'a-led').symlink_to(led, target_is_directory=True)
    (root / 'b-level').symlink_to(led / 'brightness')
    (root / 'c-trigger').symlink_to(led / 'trigger')
    (root / 'notes.txt').write_text('Ordinary file beside LED metadata. No outputs changed.\n')


def exercise(session, root):
    session.observe('01-led-strips', ['a-led', 'b-level', 'LED'])
    session.click(848, 201)
    session.observe('02-mixed-folder', ['a-led', 'b-level', 'c-trigger', 'notes.txt'])
    x, y = session.word('02-mixed-folder', 'a-led')
    session.click(x + 32, y + 55)
    session.observe('03-led-inspector', ['Measurements and limits', 'Reported LED brightness', 'Maximum brightness', 'Trigger/choices'])
    private.run_checked([str(session.pointer), 'scroll', '1280', '800', '1050', '600', '700'], env=session.env)
    session.observe('03-led-trigger-details', ['LED trigger and choices', 'observation policy'])
    session.key('Escape')
    session.observe('04-browser-return', ['a-led', 'b-level', 'c-trigger'])
    x, y = session.word('04-browser-return', 'b-level')
    session.click(x + 16, y, delay=0.05)
    session.click(x + 16, y)
    session.observe('05-level-inspector', ['attribute', 'Measurements and limits', 'Reported LED brightness', 'Maximum brightness'])
    session.key('Escape')
    session.observe('06-final-folder', ['a-led', 'b-level', 'notes.txt'])


def main():
    args = application_arguments(__doc__, Path('target/release/clip-file-manager'))
    output, root = media_fixture_directory(args)
    fixtures(root)
    return run_application_acceptance(args, output, root, exercise, 'LED read-only observations')


if __name__ == '__main__':
    raise SystemExit(main())
