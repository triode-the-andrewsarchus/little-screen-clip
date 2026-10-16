#!/usr/bin/env python3
"""Read-only sensor/cooling strip acceptance on private Weston/Niri."""
from pathlib import Path
import json
import subprocess
from itertools import islice
from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance


def fixtures(root, binary):
    devices = sorted(islice(Path('/sys/class/hwmon').iterdir(), 16))
    sensor = next(device for device in devices if (device / 'temp1_input').is_file())
    cpu = sensor
    # Fixture discovery uses the same isolated reader as the application. Query
    # only identity attributes here, rather than sampling unused sensor channels.
    for index, device in enumerate(devices):
        output = root / f'.hwmon-name-{index}.json'
        try:
            subprocess.run([str(binary.resolve()), '--internal-system-inspector', str(device / 'name'), str(output)], check=True, timeout=3)
            document = json.loads(output.read_bytes())
        except (subprocess.SubprocessError, OSError, ValueError):
            continue
        if any(fact['source'] == document['path'] and fact['origin'] == 'documented sysfs attribute' and fact['outcome'] == 'Observed' and not fact.get('retained') and fact.get('value') == 'k10temp' for fact in document['facts']):
            cpu = device
            break
    cooling = next(Path('/sys/class/thermal').glob('cooling_device*'))
    for name, target in [('a-sensors', sensor), ('b-board', cpu), ('c-cooling', cooling), ('d-reading', sensor / 'temp1_input')]:
        (root / name).symlink_to(target, target_is_directory=target.is_dir())
    (root / 'notes.txt').write_text('Ordinary file beside read-only measurements.\n')
    output = root / '.telemetry-observation.json'
    subprocess.run([str(binary.resolve()), '--internal-system-inspector', str(sensor), str(output)], check=True, timeout=3)
    assert json.loads(output.read_bytes())['telemetry']['series']


def inspect(session, observation, name):
    x, y = session.word(observation, name)
    session.click(x + 32, y + 55)


def exercise(session, root):
    session.observe('01-sensor-strips', ['a-sensors', 'b-board'])
    session.click(848, 201)
    session.observe('02-mixed-folder', ['a-sensors', 'b-board', 'c-cooling', 'd-reading'])
    inspect(session, '02-mixed-folder', 'a-sensors')
    session.observe('03-sensor-inspector', ['Measurements and limits', 'Input', 'Display span'])
    session.key('Escape')
    session.observe('04-browser-return', ['a-sensors', 'c-cooling'])
    inspect(session, '04-browser-return', 'c-cooling')
    session.observe('05-cooling-inspector', ['Thermal cooling device', 'Current state', 'Maximum state', 'Ordinal cooling state'])
    session.key('Escape')
    session.observe('06-browser-return', ['b-board', 'd-reading'])
    x, y = session.word('06-browser-return', 'd-reading')
    session.click(x + 16, y, delay=0.05)
    session.click(x + 16, y)
    session.observe('07-attribute-inspector', ['attribute', 'Measurements and limits', 'Input'])
    session.key('Escape')
    session.observe('08-final-folder', ['a-sensors', 'b-board', 'c-cooling', 'd-reading'])


def main():
    args = application_arguments(__doc__, Path('target/release/clip-file-manager'))
    output, root = media_fixture_directory(args)
    fixtures(root, args.binary)
    return run_application_acceptance(args, output, root, exercise, 'hwmon and thermal measurements')


if __name__ == '__main__':
    raise SystemExit(main())
