#!/usr/bin/env python3
"""Read-only PCI/display preview acceptance using the private Weston/Niri runner."""
from pathlib import Path
import json
import subprocess
import time
import ui_acceptance_capture as private
from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance


def fixtures(root, binary):
    for connector in sorted(Path('/sys/class/drm').iterdir()):
        if '-' not in connector.name or not (connector / 'edid').is_file():
            continue
        observation = root / '.display-observation.json'
        subprocess.run([str(binary.resolve()), '--internal-system-inspector', str(connector), str(observation)], check=True, timeout=3)
        document = json.loads(observation.read_bytes())
        if not any(fact['label'] == 'Monitor name' for fact in document['facts']):
            continue
        card = Path('/sys/class/drm') / connector.name.split('-', 1)[0]
        gpu = (card / 'device').resolve()
        (root / 'a-pci').symlink_to(gpu, target_is_directory=True)
        (root / 'b-display').symlink_to(connector, target_is_directory=True)
        (root / 'c-card').symlink_to(card, target_is_directory=True)
        (root / 'notes.txt').write_text('Ordinary file beside read-only hardware observations.\n')
        return
    raise RuntimeError('No local connector with an observed monitor name; use synthetic EDID tests on headless systems')


def exercise(session, root):
    session.observe('01-display-strips', ['a-pci', 'b-display'])
    session.click(848, 201)
    session.observe('02-mixed-folder', ['a-pci', 'b-display', 'notes.txt'])
    x, y = session.word('02-mixed-folder', 'a-pci')
    session.click(x + 32, y + 55)
    session.observe('03-pci-inspector', ['PCI vendor', 'PCI device'])
    session.scroll_to_visible('03b-pci-address-components', ['PCI domain', 'PCI bus', 'PCI device number', 'PCI function'])
    session.key('Escape')
    session.observe('04-folder-return', ['a-pci', 'b-display'])
    x, y = session.word('04-folder-return', 'b-display')
    session.click(x + 32, y + 55)
    session.observe('05-connector-inspector', ['Connection status', 'Encoder association'])
    for _ in range(24):
        private.run_checked([str(session.pointer), 'scroll', '1280', '800', '700', '600', '220'], env=session.env)
        time.sleep(0.2)
        text = session.capture_text('06-monitor-metadata', session.output / '06-monitor-metadata.png', ['Monitor name'])
        if 'monitorname' in ''.join(text.lower().split()):
            break
    session.observe('06-monitor-metadata', ['EDID', 'Monitor name'])
    session.key('Escape')
    session.observe('07-final-folder', ['a-pci', 'b-display', 'notes.txt'])
    session.scroll_to_visible('08-card-row', ['c-card'], step=150, attempts=5)
    x, y = session.word('08-card-row', 'c-card')
    session.click(x + 32, y + 55)
    session.observe('09-card-inspector', ['DRM primary access node'])
    session.scroll_to_visible('10-card-sibling-inventory', ['DRM sibling inventory', 'observed sibling', '32 retained'], step=220, attempts=24)
    session.key('Escape')
    session.observe('11-card-folder-return', ['a-pci', 'b-display', 'c-card'])


def main():
    args = application_arguments(__doc__, Path('target/release/clip-file-manager'))
    output, root = media_fixture_directory(args)
    fixtures(root, args.binary)
    return run_application_acceptance(args, output, root, exercise, 'PCI and display metadata')


if __name__ == '__main__':
    raise SystemExit(main())
