#!/usr/bin/env python3
"""Last-known matches remain opt-in and metadata-only across source replacement."""
from pathlib import Path
import json
import re
import shlex
import sqlite3
import subprocess
import time
import tomllib
import wave

import ui_acceptance_capture as private
from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance, wait_for
from file_manager_maintenance_acceptance import search_environment, scroll, word, enroll_folder
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



def payload_reads(logs, target):
    calls = ('read(', 'pread64(', 'readv(', 'preadv(', 'preadv2(', 'mmap(')
    descriptor = re.compile(r'<' + re.escape(str(target)) +
                            r'(?: \(deleted\))?(?:<(?:char|block) [^>]+>)?>')
    return [line for text in logs.values() for line in text.splitlines()
            if descriptor.search(line) and any(call in line for call in calls)]


def exercise_live(session, root):
    source, browser = root/'Indexed', root/'Browser'
    index, (_, _, vectors) = enroll_folder(session, source)
    session.observe('index-source', ['tone.wav'])
    assert vectors == 2
    pause_indexing(session, True, 'pause-before-file-replacement')
    session.address(browser)
    submit(session, ':emb:"tone.wav";scope:indexed')
    session.observe('current-result', ['tone.wav', 'Semantic candidates'])
    x, y = session.word('current-result', 'tone.wav', minimum_y=250)
    target = source/'tone.wav'
    original_inode = target.stat().st_ino
    target.rename(root/'saved-source.wav')
    # No file exists at the selected address at this boundary. Reads through
    # the old handle now carry saved-source.wav, avoiding an identity ambiguity.
    offsets = {path.name:path.stat().st_size for path in session.output.glob('source-trace.*')}
    target.write_bytes(b'Replaced ordinary file: do not decode this payload.')
    assert target.stat().st_ino != original_inode
    with sqlite3.connect(index) as db:
        assert db.execute('SELECT available,inode FROM roots').fetchone() == (1, source.stat().st_ino)
    session.click(x + 10, y)
    session.key('c', 'ctrl')
    command = ['wl-paste', '--no-newline', '--type', 'text/uri-list']
    wait_for(lambda: subprocess.run(command, env=session.env, capture_output=True, text=True,
                                   timeout=2).stdout.strip() == target.as_uri(),
             'ordinary file-URL reference export', 10)
    session.observe('reference-only-copy', ['1 files held for copying'])
    x, y = session.word('reference-only-copy', 'tone.wav', minimum_y=250)
    session.click(x + 10, y)
    session.click(x + 10, y, delay=0.05)
    session.observe('changed-file-open-rejected', ['Search result changed', 'Back to folder'])
    session.key('Escape')
    session.observe('browser-return', ['Name', 'Size'])
    session.key('F5')
    session.observe('refresh-omits-old-revision', ['0 of 0 matching entries', 'Semantic candidates'])
    observed = chunks(session, offsets)
    accesses = payload_reads(observed, target)
    assert not accesses, accesses
    assert target.read_bytes() == b'Replaced ordinary file: do not decode this payload.'
    (session.output/'live-source-trace.json').write_text(json.dumps({
        'traced_processes':len(observed), 'replacement_payload_reads_or_mappings':0,
        'trace_offsets':offsets,
        'reference_export_is_metadata_only':True, 'content_open_rejected':True,
        'refresh_omits_old_revision':True, 'root_still_available_at_action':True,
        'scope':'Ordinary live file replacement, cached preview and audio-studio open; file URLs are references, not identity-bound content snapshots.',
    }, indent=2))


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
    wrapper.write_text('#!/bin/sh\nexec strace -D -ff -qq -yy -s 0 -e trace=openat,openat2,read,pread64,readv,preadv,preadv2,mmap -o ' + shlex.quote(str(output / 'source-trace')) + ' ' + shlex.quote(str(binary)) + ' "$@"\n')
    wrapper.chmod(0o700)
    args.binary = wrapper


def main():
    repository = Path(__file__).resolve().parent.parent
    def configure(parser):
        parser.add_argument('--live-replacement', action='store_true', help='Replace a selected file while its indexed root stays available')
    args = application_arguments(__doc__, repository / 'target/debug/clip-file-manager', configure)
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
    scenario = exercise_live if args.live_replacement else exercise
    scope = ('Live selected-file replacement: file references remain metadata-only, guarded content open rejects replacement and query refresh omits the stale revision' if args.live_replacement else
             'Persistent opt-in, cached unavailable labels, no replacement payload reads/copy/open, disable and reconnect to original identity')
    return run_application_acceptance(args, output, root, scenario, scope, search_environment)


if __name__ == '__main__':
    raise SystemExit(main())
