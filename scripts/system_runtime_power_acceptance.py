#!/usr/bin/env python3
"""Read-only runtime-power accounting on private Weston/Niri."""
from pathlib import Path
import ui_acceptance_capture as private
from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance


def exercise(session, root):
    session.observe('01-power-folder', ['a-power', 'b-active-time', 'Device power management'])
    x, y = session.word('01-power-folder', 'b-active-time')
    session.click(x + 16, y, delay=0.05)
    session.click(x + 16, y)
    session.observe('02-duration-inspector', ['Device power management attribute', 'Runtime active time', 'ms', 'Back to folder'])
    session.key('Escape')
    session.observe('03-browser-return', ['a-power', 'b-active-time'])
    x, y = session.word('03-browser-return', 'a-power')
    session.click(x + 16, y, delay=0.05)
    session.click(x + 16, y)
    session.observe('04-power-directory', ['autosuspend_delay_ms', 'control'])
    private.run_checked([str(session.pointer), 'scroll', '1280', '800', '1000', '450', '500'], env=session.env)
    session.observe('04b-accounting-attributes', ['runtime_active_time', 'runtime_status'])
    private.run_checked([str(session.pointer), 'scroll', '1280', '800', '1000', '450', '500'], env=session.env)
    session.observe('04c-suspended-accounting', ['runtime_suspended_time'])
    session.key('Left', 'alt')
    session.observe('05-history-return', ['a-power', 'b-active-time'])


def main():
    args = application_arguments(__doc__, Path('target/release/clip-file-manager'))
    output, root = media_fixture_directory(args)
    power = Path('/sys/devices/virtual/net/lo/power')
    assert (power / 'runtime_active_time').is_file()
    (root / 'a-power').symlink_to(power, target_is_directory=True)
    (root / 'b-active-time').symlink_to(power / 'runtime_active_time')
    (root / 'notes.txt').write_text('Ordinary file beside read-only power accounting.\n')
    return run_application_acceptance(args, output, root, exercise, 'runtime-power accounting')


if __name__ == '__main__':
    raise SystemExit(main())
