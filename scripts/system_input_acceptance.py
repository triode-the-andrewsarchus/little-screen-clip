#!/usr/bin/env python3
"""Passive input/HID previews on private Weston/Niri, without reading events."""
from pathlib import Path
import json
import subprocess
import time
import ui_acceptance_capture as private
from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance


def fixtures(root, binary):
    event = next(path for path in sorted(Path('/sys/class/input').iterdir())
                 if path.name.startswith('event') and any((parent / 'subsystem').resolve().name == 'hid' for parent in path.resolve().parents))
    (root / 'a-event').symlink_to(event, target_is_directory=True)
    observation = root / '.input-observation.json'
    subprocess.run([str(binary.resolve()), '--internal-system-inspector', str(event), str(observation)], check=True, timeout=3)
    document = json.loads(observation.read_text())
    raw = next(Path(node['path']) for node in document['topology']['nodes'] if 'Raw HID interface' in node['label'])
    (root / 'b-hid').symlink_to(raw, target_is_directory=True)
    (root / 'notes.txt').write_text('Ordinary file beside passive input metadata.\n')


def exercise(session, root):
    session.observe('01-input-strips', ['a-event', 'b-hid', 'Event interface'])
    session.click(848, 201)
    session.observe('02-mixed-folder', ['a-event', 'b-hid', 'notes.txt'])
    x, y = session.word('02-mixed-folder', 'a-event')
    session.click(x + 32, y + 55)
    session.observe('03-event-inspector', ['Event interface', 'Input name', 'Input event categories'])
    session.key('Escape')
    session.observe('04-folder-return', ['a-event', 'b-hid'])
    x, y = session.word('04-folder-return', 'b-hid')
    session.click(x + 32, y + 55)
    session.observe('05-hid-inspector', ['Raw HID interface', 'HID name'])
    for _ in range(16):
        private.run_checked([str(session.pointer), 'scroll', '1280', '800', '700', '600', '160'], env=session.env)
        time.sleep(0.15)
        text = session.capture_text('06-report-metadata', session.output / '06-report-metadata.png', ['HID declared reports'])
        if 'hiddeclaredreports' in ''.join(text.lower().split()):
            break
    session.observe('06-report-metadata', ['HID declared reports'])
    session.key('Escape')
    session.observe('07-final-folder', ['a-event', 'b-hid', 'notes.txt'])


def main():
    args = application_arguments(__doc__, Path('target/release/clip-file-manager'))
    output, root = media_fixture_directory(args)
    fixtures(root, args.binary)
    return run_application_acceptance(args, output, root, exercise, 'passive input and HID previews')


if __name__ == '__main__':
    raise SystemExit(main())
