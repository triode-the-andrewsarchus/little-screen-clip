#!/usr/bin/env python3
"""Read-only USB preview acceptance on the private Weston/Niri desktop."""
from pathlib import Path
import json
import subprocess
import shutil
import ui_acceptance_capture as private
from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance


def fixtures(root, binary):
    for device in sorted(Path('/sys/bus/usb/devices').iterdir()):
        if device.name.startswith('usb') or not (device / 'idVendor').is_file():
            continue
        output = root / '.usb-observation.json'
        subprocess.run([str(binary.resolve()), '--internal-system-inspector', str(device), str(output)], check=True, timeout=3)
        document = json.loads(output.read_bytes())
        if document.get('topology'):
            (root / 'a-usb').symlink_to(device, target_is_directory=True)
            (root / 'notes.txt').write_text('Ordinary file next to USB metadata.\n')
            return
        output.unlink()
    raise RuntimeError('No local USB device available; synthetic Rust serial fixtures remain hardware independent')


def exercise(session, root):
    session.observe('01-usb-strip', ['a-usb', 'USB device'])
    session.click(848, 201)
    session.observe('02-usb-mixed', ['a-usb', 'notes.txt'])
    x, y = session.word('02-usb-mixed', 'a-usb')
    session.click(x + 32, y + 55)
    session.observe('03-usb-inspector', ['USB vendor ID', 'USB product ID', 'Negotiated USB speed'])
    (session.output / 'files-owned-binary').unlink()
    session.click(284, 172)
    session.observe('03b-refresh-unlinked-executable', ['USB vendor ID', 'Negotiated USB speed'])
    refreshed = (session.output / '03b-refresh-unlinked-executable.txt').read_text()
    assert 'Observation incomplete' not in refreshed, 'helper failed after executable unlink'
    assert not (session.output / 'files-owned-binary').exists()
    session.scroll_to_visible('03c-usb-interface-details', ['Alternate setting', 'Interface protocol'])
    session.scroll_to_visible('04-usb-relationships', ['Related kernel objects'], step=150, attempts=12)
    session.scroll_to_visible('04b-usb-topology', ['USB topology', 'Coverage'], step=-150, attempts=12)
    session.key('Escape')
    session.observe('05-usb-return', ['a-usb', 'notes.txt'])


def main():
    args = application_arguments(__doc__, Path('target/release/clip-file-manager'))
    output, root = media_fixture_directory(args)
    fixtures(root, args.binary)
    owned_binary = output / 'files-owned-binary'
    shutil.copy2(args.binary, owned_binary)
    args.binary = owned_binary
    return run_application_acceptance(args, output, root, exercise, 'USB topology and metadata')


if __name__ == '__main__':
    raise SystemExit(main())
