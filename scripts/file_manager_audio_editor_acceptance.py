#!/usr/bin/env python3
"""Owned audio editor copies and guarded source rejection in the private desktop."""
import json
import os
import sys
from pathlib import Path
import file_manager_audio_studio_source_acceptance as base
from file_manager_channel_undo_source_acceptance import play_fresh_audio


def launches(session):
    report = session.output / 'audio-editor.jsonl'
    return [json.loads(line) for line in report.read_text().splitlines()] if report.exists() else []


def exercise(session, root):
    index = base.open_checked_audio(session, root)
    session.key('c')
    session.observe('channel-tools', ['Apply to'])
    session.click(220, 386)
    session.key('2')
    session.observe('derived-ready', ['Channel edit applied'])
    replacement, offsets = base.replace_audio_root(session, root)
    session.click(1115, 188)
    base.wait_for(lambda: len(launches(session)) == 1, 'editor reads an owned session copy')
    session.observe('derived-editor-copy', ['session copy'])
    session.key('z', 'ctrl')
    session.click(1115, 188)
    session.observe('original-editor-rejected', ['Could not open audio editor', 'Search result changed'])
    assert len(launches(session)) == 1
    session.key('y', 'ctrl')
    session.observe('redo-restored', ['Apply to'])
    play_fresh_audio(session, 'redo restores the playable owned input before editor handoff')
    session.click(1115, 188)
    base.wait_for(lambda: len(launches(session)) == 2, 'redo launches a second owned copy')
    session.observe('redo-editor-copy', ['session copy'])
    denied = base.payload_accesses(base.chunks(session, offsets), replacement)
    assert not denied, denied
    assert not index.exists()
    session.click(160, 118)
    session.observe('discard-edits', ['Unsaved edits', 'Discard edits'])
    session.click(*session.word('discard-edits', 'Discard', minimum_y=300))
    session.observe('returned-browser', ['Folders', 'tone.wav'])
    copies = launches(session)
    assert len({record['path'] for record in copies}) == 2
    for record in copies:
        path = Path(record['path'])
        assert path != replacement and path.name.startswith('audio-session-')
        assert path.exists() and path.stat().st_size == record['bytes'] > 1000
        assert record['wave'] and path.stat().st_mode & 0o077 == 0
        # The recording shim has exited and the viewer was discarded. Its input
        # must remain available to a daemonized/forwarded editor.
        path.unlink()
    (session.output / 'audio-editor-source.json').write_text(json.dumps({
        'owned_copy_launches':len(copies), 'original_replacement_rejected':True,
        'replacement_payload_accesses':len(denied), 'copies_survive_viewer_close':True,
        'copies_private':True, 'no_index_created':True,
    }, indent=2))


def environment(output, env, children):
    tools = output / 'tools'
    tools.mkdir()
    editor = tools / 'audacity'
    editor.write_text('#!' + sys.executable + '\n' +
        'import json, os, sys\n'
        'from pathlib import Path\n'
        'path = Path(sys.argv[1])\n'
        'with path.open("rb") as audio: header = audio.read(12)\n'
        'record = {"path":str(path), "bytes":path.stat().st_size, "wave":header[:4] == b"RIFF" and header[8:] == b"WAVE"}\n'
        'with open(os.environ["CLIP_AUDIO_EDITOR_REPORT"], "a") as report: report.write(json.dumps(record) + "\\n")\n')
    editor.chmod(0o700)
    env['PATH'] = str(tools) + os.pathsep + env['PATH']
    env['CLIP_AUDIO_EDITOR_REPORT'] = str(output / 'audio-editor.jsonl')
    return base.start_private_audio(output, env, children)


if __name__ == '__main__':
    raise SystemExit(base.main(exercise,
        'Guarded audio editor handoffs, private owned copies and zero replacement reads',
        prepare_environment=lambda env, children: environment(Path(env['XDG_RUNTIME_DIR']).parent, env, children)))
