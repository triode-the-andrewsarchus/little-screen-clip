#!/usr/bin/env python3
"""Last-known matches remain opt-in and metadata-only across source replacement."""
from pathlib import Path
import json
import shlex
import sqlite3
import time
import tomllib
import wave

import ui_acceptance_capture as private
from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance, wait_for
from file_manager_maintenance_acceptance import search_environment, scroll, word
from file_manager_search_settings_acceptance import click_word
from file_manager_search_acceptance import submit, pause_indexing
from file_manager_reuse_acceptance import inventory


def settings(session, label):
    session.click(151, 70)
    session.observe(label + '-edit', ['Search settings'])
    click_word(session, label + '-edit', 'Search')


def offline_option(session, enabled, label):
    settings(session, label)
    if not getattr(session, 'offline_expanded', False):
        session.scroll_to_visible(label + '-offline-header', ['Offline results'], position=(500, 450))
        anchor = word(session, label + '-offline-header', 'Offline')
        session.click(anchor[0] + 10, anchor[1])
        session.offline_expanded = True
    session.scroll_to_visible(label + '-offline-checkbox', ['Show offline results'], position=(500, 450))
    anchor = word(session, label + '-offline-checkbox', 'Show')
    session.click(anchor[0] + 10, anchor[1])
    scroll(session, -800)
    session.observe(label + '-top', ['Pause background indexing'])
    click_word(session, label + '-top', 'Save')
    config = session.output / 'scratch/config.toml'
    wait_for(lambda: tomllib.loads(config.read_text())['file_manager']['search']['show_offline_results'] == enabled, 'persist offline result preference')
    click_word(session, label + '-top', 'Close')


def refresh_root(session, index, label):
    settings(session, label)
    session.scroll_to_visible(label + '-refresh', ['Refresh index'], position=(500, 450))
    anchor = word(session, label + '-refresh', 'Refresh')
    session.click(anchor[0] + 10, anchor[1])
    scroll(session, -800)
    session.observe(label + '-top', ['Pause background indexing'])
    click_word(session, label + '-top', 'Close')


def chunks(session, offsets=None):
    result = {}
    for path in session.output.glob('source-trace.*'):
        result[path.name] = path.read_bytes()[(offsets or {}).get(path.name, 0):].decode(errors='replace')
    return result


def payload_accesses(logs, target):
    return [line for text in logs.values() for line in text.splitlines()
        if str(target) in line and (('read(' in line or 'pread64(' in line)
        or ('openat' in line and 'O_PATH' not in line))]


def exercise(session, root):
    source, browser = root / 'Indexed', root / 'Browser'
    session.address(source)
    session.observe('index-source', ['tone.wav'])
    index = Path(session.env['XDG_DATA_HOME']) / 'little-screen-clip/search/index.sqlite'
    session.click(993, 118)
    session.key('Escape')
    generation, _, vectors = inventory(index)
    assert vectors == 2
    expected_inode = source.stat().st_ino
    pause_indexing(session, True, 'pause-before-disconnect')
    session.address(browser)
    session.observe('separate-browser', ['Start.txt'])
    source.rename(root / 'Original')
    source.mkdir()
    canary = source / 'tone.wav'
    canary.write_bytes(b'Outside replacement: do not decode or copy this file.')
    refresh_root(session, index, 'detect-replacement')
    def unavailable():
        with sqlite3.connect(index) as db:
            return db.execute('SELECT state,available,inode FROM roots').fetchone() == ('offline', 0, expected_inode)
    wait_for(unavailable, 'retain original root identity and mark replacement unavailable')
    assert payload_accesses(chunks(session), canary), 'Trace failed to capture the earlier audio read'
    offsets = {path.name: path.stat().st_size for path in session.output.glob('source-trace.*')}
    submit(session, ':emb:"tone.wav";scope:indexed')
    session.observe('offline-hidden-by-default', ['0 of 0 matching entries'])
    session.key('Escape')
    submit(session, ':content:"Violet";scope:indexed')
    session.observe('unavailable-literal-scope-is-incomplete', ['0 matching entries', '1 unsupported/incomplete reads'])
    session.key('Escape')
    offline_option(session, True, 'enable-offline')
    submit(session, ':emb:"tone.wav";scope:indexed')
    session.observe('last-known-metadata', ['tone.wav', 'Unavailable', 'last-known', 'metadata list'])
    x, y = session.word('last-known-metadata', 'tone.wav', minimum_y=250)
    session.click(x + 10, y)
    session.key('c', 'ctrl')
    session.observe('offline-copy-rejected', ['Reconnect and refresh before copying'])
    session.click(x + 10, y)
    session.click(x + 10, y, delay=0.05)
    session.observe('offline-open-rejected', ['Reconnect and refresh before opening'])
    observed = chunks(session, offsets)
    assert not payload_accesses(observed, canary), payload_accesses(observed, canary)
    (session.output / 'offline-source-trace.json').write_text(json.dumps({'traced_processes': len(observed), 'replacement_payload_accesses': 0}, indent=2))
    assert canary.read_bytes() == b'Outside replacement: do not decode or copy this file.'
    session.key('Escape')
    offline_option(session, False, 'disable-offline')
    submit(session, ':emb:"tone.wav";scope:indexed')
    session.observe('offline-hidden-after-disable', ['0 of 0 matching entries'])
    session.key('Escape')
    source.rename(root / 'Replacement')
    (root / 'Original').rename(source)
    pause_indexing(session, False, 'resume-original')
    refresh_root(session, index, 'refresh-original')
    inventory(index, generation)
    submit(session, ':emb:"tone.wav";scope:indexed')
    text = session.observe('reconnected-current-result', ['tone.wav', 'Semantic candidates'])
    assert 'last-known' not in text.lower()


def trace_application(args, output):
    binary = args.binary.resolve()
    wrapper = output / 'trace-files.sh'
    wrapper.write_text('#!/bin/sh\nexec strace -D -ff -qq -yy -s 0 -e trace=openat,openat2,read,pread64 -o ' + shlex.quote(str(output / 'source-trace')) + ' ' + shlex.quote(str(binary)) + ' "$@"\n')
    wrapper.chmod(0o700)
    args.binary = wrapper


def main():
    repository = Path(__file__).resolve().parent.parent
    args = application_arguments(__doc__, repository / 'target/debug/clip-file-manager')
    output, root = media_fixture_directory(args)
    (root / 'Indexed').mkdir()
    (root / 'Browser').mkdir()
    (root / 'Browser/Start.txt').write_text('Independent browser location.')
    (root / 'Indexed/One.txt').write_text('Violet lighthouse beside a harbour.')
    with wave.open(str(root / 'Indexed/tone.wav'), 'wb') as audio:
        audio.setnchannels(1)
        audio.setsampwidth(2)
        audio.setframerate(16000)
        audio.writeframes(b'\0\0' * 1600)
    trace_application(args, output)
    return run_application_acceptance(args, output, root, exercise,
        'Persistent opt-in, cached unavailable labels, no replacement payload reads/copy/open, disable and reconnect to original identity', search_environment)


if __name__ == '__main__':
    raise SystemExit(main())
