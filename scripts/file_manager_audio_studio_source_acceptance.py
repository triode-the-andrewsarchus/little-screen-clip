#!/usr/bin/env python3
"""Checked Audio Studio playback accepts the selected source and rejects replacements."""
import json
from pathlib import Path
from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance, wait_for
from file_manager_media_audio import start_private_audio
from file_manager_offline_acceptance import chunks, payload_accesses, trace_application
from file_manager_phase5_acceptance import audio_tail
from file_manager_search_acceptance import submit
from file_manager_studio_source_acceptance import open_result
from ui_acceptance_fixtures import wav_fixture


def open_checked_audio(session, root):
    session.observe('initial', ['tone.wav'])
    index = Path(session.env['XDG_DATA_HOME']) / 'little-screen-clip/search/index.sqlite'
    assert not index.exists()
    submit(session, ':*.wav tone;scope:recursive')
    session.observe('audio-result', ['tone.wav', '1 matching entries'])
    open_result(session, 'audio-result', 'tone.wav')
    session.observe('checked-audio-studio', ['Back to folder', 'Inspect & Slice'])
    return index


def replace_audio_root(session, root):
    offsets = {path.name:path.stat().st_size for path in session.output.glob('source-trace.*')}
    root.rename(session.output / 'original-source')
    root.mkdir()
    replacement = root / 'tone.wav'
    wav_fixture(replacement, 2, 'stereo')
    return replacement, offsets


def exercise(session, root):
    index = open_checked_audio(session, root)
    session.key('space')
    wait_for(lambda: any(abs(value) > 100 for value in audio_tail(session)[0]),
             'checked asynchronous decoder produces audio on the private sink')
    session.key('space')
    session.observe('paused-selected-audio', ['Back to folder'])
    session.key('c')
    session.observe('channel-tools', ['Apply to'])
    session.click(220, 386)  # Track target in the private runner's fixed-size studio.
    target, offsets = replace_audio_root(session, root)
    session.key('space')
    session.observe('rejected-replacement-playback', ['Search result changed'])
    session.key('2')
    session.observe('rejected-replacement-channel-edit', ['Could not apply channel edit', 'Search result changed'])
    denied = payload_accesses(chunks(session, offsets), target)
    assert not denied, denied
    assert not index.exists()
    (session.output / 'audio-studio-source.json').write_text(json.dumps({
        'selected_source_played': True,
        'replacement_playback_visibly_rejected': True,
        'replacement_channel_edit_visibly_rejected': True,
        'replacement_payload_accesses': len(denied), 'no_index_created': True,
    }, indent=2))


def main(exercise=exercise, scope="Checked Audio Studio playback, then visible playback/channel rejection with zero replacement payload accesses", prepare_environment=None):
    repo = Path(__file__).resolve().parent.parent
    args = application_arguments(__doc__, repo / 'target/debug/clip-file-manager')
    output, root = media_fixture_directory(args)
    wav_fixture(root / 'tone.wav', 5, 'stereo')
    trace_application(args, output)
    return run_application_acceptance(args, output, root, exercise,
        scope,
        prepare_environment=prepare_environment or (lambda env, children: start_private_audio(output, env, children)))


if __name__ == '__main__':
    raise SystemExit(main())
