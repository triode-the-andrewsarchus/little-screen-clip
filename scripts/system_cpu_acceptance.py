#!/usr/bin/env python3
"""Read-only CPU/cache/policy preview acceptance on private Weston/Niri."""
from pathlib import Path
import subprocess
import os
import json
from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance


def fixtures(root, binary):
    cpu_root = Path('/sys/devices/system/cpu')
    logical = next(path for path in sorted(cpu_root.glob('cpu[0-9]*')) if (path / 'topology').is_dir() and int(path.name[3:]) in os.sched_getaffinity(0))
    caches = sorted((logical / 'cache').glob('index[0-9]*'))
    if not caches or not (logical / 'cpufreq').exists():
        raise RuntimeError('Local CPU has no cache or frequency policy; use the synthetic CPU fixtures')
    for name, target in [('a-cpu', logical), ('b-cache', caches[-1]), ('c-policy', logical / 'cpufreq'), ('d-online', cpu_root / 'online')]:
        (root / name).symlink_to(target, target_is_directory=target.is_dir())
    (root / 'notes.txt').write_text('Ordinary browsing beside CPU metadata.\n')
    output = root / '.cpu-observation.json'
    subprocess.run([str(binary.resolve()), '--internal-system-inspector', str(logical), str(output)], check=True, timeout=3)
    observation = json.loads(output.read_bytes())
    assert observation['cpu']['bins']


def inspect(session, observation, name):
    x, y = session.word(observation, name)
    session.click(x + 32, y + 55)


def exercise(session, root):
    session.observe('01-cpu-strips', ['a-cpu', 'b-cache'])
    session.click(848, 201)
    session.observe('02-cpu-mixed', ['a-cpu', 'b-cache', 'c-policy', 'd-online'])
    inspect(session, '02-cpu-mixed', 'a-cpu')
    session.observe('03-cpu-inspector', ['CPU membership', 'Package', 'Core', 'Online CPUs'])
    session.scroll_to_visible('03b-cpu-identification', ['CPU vendor', 'CPU model', 'CPUID base capabilities'])
    session.key('Escape')
    session.observe('04-browser-return', ['a-cpu', 'b-cache', 'c-policy'])
    inspect(session, '04-browser-return', 'b-cache')
    session.observe('05-cache-inspector', ['CPU cache', 'Shared CPUs', 'Unified'])
    session.key('Escape')
    session.observe('06-browser-return', ['c-policy', 'd-online'])
    inspect(session, '06-browser-return', 'c-policy')
    session.observe('07-policy-inspector', ['CPU frequency policy', 'Policy CPUs', 'GHz'])
    session.key('Escape')
    session.observe('08-browser-return', ['a-cpu', 'd-online'])
    x, y = session.word('08-browser-return', 'd-online')
    session.click(x + 16, y, delay=0.05)
    session.click(x + 16, y)
    session.observe('09-mask-inspector', ['CPU attribute', 'CPU membership', 'Online CPUs'])
    session.key('Escape')
    session.observe('10-final-folder', ['a-cpu', 'b-cache', 'c-policy', 'd-online'])


def main():
    args = application_arguments(__doc__, Path('target/release/clip-file-manager'))
    output, root = media_fixture_directory(args)
    fixtures(root, args.binary)
    return run_application_acceptance(args, output, root, exercise, 'CPU topology/cache/frequency membership')


if __name__ == '__main__':
    raise SystemExit(main())
