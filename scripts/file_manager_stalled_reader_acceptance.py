#!/usr/bin/env python3
"""Healthy-root queries and cancellation while an owned index reader is held."""
import json
import os
from pathlib import Path
import sqlite3
import subprocess
import time
import ui_acceptance_capture as private

from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance, wait_for
from file_manager_frontmatter_acceptance import environment
from file_manager_quality_acceptance import search_timings
from file_manager_reuse_acceptance import inventory
from file_manager_search_acceptance import submit


def retire_reader(session, slow, healthy, index, pid, stage):
    session.key('Escape')
    session.address(slow)
    session.click(993, 118)
    session.observe('remove-held-root', ['Remove indexed folder'])
    session.click(*session.word('remove-held-root', 'Remove', last=True))
    wait_for(lambda: not Path(f'/proc/{pid}').exists(), 'removed root cancels and reaps its owned reader', 5)
    with sqlite3.connect(index) as db:
        assert db.execute('SELECT COUNT(*) FROM roots').fetchone()[0] == 1
        assert [os.fsdecode(path) for path, in db.execute('SELECT path FROM paths')] == [str(healthy / 'Healthy.txt')]
    if stage == 'watch':
        # An abandoned registration may have modified the shared instance.
        # Healthy coverage must reconcile after retiring that instance.
        _, paths, vectors = inventory(index)
        assert paths == [str(healthy / 'Healthy.txt')] and vectors == 1
    session.address(healthy)
    submit(session, ':emb:"Healthy.txt"')
    session.observe('healthy-after-reader-removal', ['Healthy.txt', 'Semantic candidates'])


def query_while_held(session, healthy, pid, release, restarted):
    session.address(healthy)
    session.observe('healthy-during-stall', ['Healthy.txt'])
    log = session.output / ('files-resumed.log' if restarted else 'files.log')
    previous = {row['request_id'] for row in search_timings(log)}
    submit(session, ':emb:"Healthy.txt"')
    def completed():
        return [row for row in search_timings(log) if row['complete'] and row['request_id'] not in previous]
    start = time.monotonic()
    try:
        wait_for(completed, 'healthy query completes while unrelated reader is held', 5)
    finally:
        evidence = dict(operation=session.env.get('CLIP_TEST_STALL_OPERATION', 'List'),
                        wait_seconds=time.monotonic()-start, results=completed(),
                        held_pid=pid, held_process_exists=Path(f'/proc/{pid}').exists(), released=release.exists())
        (session.output / 'held-query.json').write_text(json.dumps(evidence, indent=2))
    rows = completed()
    assert len(rows) == 1 and rows[0]['succeeded'] and rows[0]['entries'] == 1, rows
    assert Path(f'/proc/{pid}').exists() and not release.exists()
    session.observe('healthy-result-while-held', ['Healthy.txt', 'Semantic candidates'])


def restart_probe(session, root, index, children, binary):
    _, paths, vectors = inventory(index)
    assert len(paths) == vectors == 2
    session.key('q', 'ctrl')
    session.process.wait(timeout=5)
    assert session.process.returncode == 0
    (session.output/'held-reader.pid').unlink(missing_ok=True)
    (session.output/'release-reader').unlink()
    session.process = children.start('files-resumed', [str(binary.resolve()), str(root/'Healthy')],
        env=session.env, cwd=session.output/'scratch', log_path=session.output/'files-resumed.log')
    private.wait_for_app_window(session.niri, session.env, session.process, session.output)
    session.key('F24')  # Refresh the private virtual keymap for the new client.
    wait_for((session.output/'held-reader.pid').exists, 'restored slow root reaches its probe hold')
    def healthy_ready():
        with sqlite3.connect(index) as db:
            return db.execute("SELECT state FROM roots WHERE path=?", [os.fsencode(root/'Healthy')]).fetchone() == ('ready',)
    wait_for(healthy_ready, 'healthy root reconciles after restart', 30)


def exercise(session, root, crash=False, stage='inventory', restart=None):
    healthy, slow = root / 'Healthy', root / 'Slow'
    slow_name = 'Slow.rtf' if stage == 'extract' else 'Slow.txt'
    release = session.output / 'release-reader'
    marker = session.output / 'held-reader.pid'
    index = Path(session.env['XDG_DATA_HOME']) / 'little-screen-clip/search/index.sqlite'
    session.address(healthy)
    session.observe('healthy-before-enrollment', ['Healthy.txt'])
    session.click(993, 118)
    session.key('Escape')
    _, paths, vectors = inventory(index)
    assert paths == [str(healthy / 'Healthy.txt')] and vectors == 1
    try:
        session.address(slow)
        session.observe('slow-before-enrollment', [slow_name])
        session.click(993, 118)
        session.key('Escape')
        if restart:
            restart(session, root, index)
        if stage == 'absence':
            _, paths, vectors = inventory(index)
            assert str(slow / slow_name) in paths and vectors == 2
            (slow / slow_name).unlink()
        wait_for(marker.exists, 'owned reader reaches the controlled stall')
        pid = int(marker.read_text())
        assert Path(f'/proc/{pid}').exists() and not release.exists()
        if stage == 'absence':
            with sqlite3.connect(index) as db:
                retained = db.execute('SELECT COUNT(*) FROM paths WHERE path=?', [os.fsencode(slow / slow_name)]).fetchone()[0]
                assert retained == 1, 'Unfinished absence check retired the path'
            (session.output / 'held-absence.json').write_text(json.dumps(dict(
                stored_paths_for_missing_source=retained, reader_pid=pid,
                gate_released=release.exists()), indent=2))
        query_while_held(session, healthy, pid, release, bool(restart))
        if crash:
            session.process.kill()
            session.process.wait(timeout=5)
            def reader_dead():
                try:
                    state = Path(f'/proc/{pid}/stat').read_text().rsplit(')', 1)[1].split()[0]
                    return state == 'Z'
                except FileNotFoundError:
                    return True
            wait_for(reader_dead, 'held child dies when Files is killed', 5)
            (session.output / 'parent-death.json').write_text(json.dumps(dict(
                application_exit=session.process.returncode, reader_dead=True,
                reader_reaped=not Path(f'/proc/{pid}').exists(), released=release.exists()), indent=2))
            assert not release.exists()
            return
        retire_reader(session, slow, healthy, index, pid, stage)
    finally:
        release.touch()


def main():
    repo = Path(__file__).resolve().parent.parent
    def options(parser):
        parser.add_argument('--crash', action='store_true')
        parser.add_argument('--restart', action='store_true', help='Hold automatic revalidation of an already indexed root on a real restart; requires --stage probe')
        parser.add_argument('--stage', choices=['inventory', 'fingerprint', 'extract', 'absence', 'watch', 'probe'], default='inventory')
    args = application_arguments(__doc__, repo / 'target/release/clip-file-manager', options)
    if args.restart and args.stage != 'probe':
        raise ValueError('--restart requires --stage probe')
    output, root = media_fixture_directory(args)
    state = {}
    for name in ['Healthy', 'Slow']:
        (root / name).mkdir()
        suffix = '.rtf' if name == 'Slow' and args.stage == 'extract' else '.txt'
        text = r'{\rtf1\ansi Slow ordinary content for the owned reader fixture.}' if suffix == '.rtf' else f'{name} ordinary content for the owned reader fixture.\n'
        (root / name / f'{name}{suffix}').write_text(text)
    library = output / 'reader-stall.so'
    subprocess.run(['cc', '-shared', '-fPIC', '-O2', '-Wall', '-Wextra', '-Werror', str(repo / 'scripts/fixtures/search_reader_stall.c'), '-ldl', '-o', str(library)], check=True)
    def prepare(env, children):
        state['children'] = children
        if args.restart:
            (output/'release-reader').touch()
        environment(env, children)
        target = root / 'Slow' if args.stage in ('inventory', 'watch', 'probe') else root / 'Slow' / ('Slow.rtf' if args.stage == 'extract' else 'Slow.txt')
        env.update(LD_PRELOAD=str(library), CLIP_TEST_STALL_DIRECTORY=str(target),
                   CLIP_TEST_STALL_MARKER=str(output / 'held-reader.pid'), CLIP_TEST_STALL_RELEASE=str(output / 'release-reader'))
        if args.stage != 'inventory':
            env['CLIP_TEST_STALL_OPERATION'] = 'Absent' if args.stage == 'absence' else args.stage.capitalize()
        if args.stage in ('absence', 'watch', 'probe'):
            env['CLIP_TEST_STALL_AT_REQUEST'] = '1'
    return run_application_acceptance(args, output, root, lambda session, folder: exercise(session, folder, args.crash, args.stage,
            (lambda session, root, index: restart_probe(session, root, index, state['children'], args.binary)) if args.restart else None),
        'Controlled held reader: healthy query within five seconds; abrupt parent death kills child' if args.crash else
        'Controlled user-space hold in one owned reader, healthy-root query completion within five seconds and removal kills/reaps the held child; not an uninterruptible kernel-I/O test', prepare,
        expected_exit_code=-9 if args.crash else 0)


if __name__ == '__main__':
    raise SystemExit(main())
