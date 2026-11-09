#!/usr/bin/env python3
"""Guarded asynchronous multi-stem export in the private desktop runner."""
import json
from pathlib import Path
import file_manager_audio_studio_source_acceptance as base
import file_manager_stem_source_acceptance as stems
from file_manager_acceptance import private


def exercise(session, root):
    index, job = stems.ready_stems(session, root)
    expected = {name: (job / f'{name}.wav').read_bytes() for name in ['vocals', 'drums', 'bass', 'other']}
    replacement, offsets = base.replace_audio_root(session, root)
    destination = session.output / 'exports'
    destination.mkdir()
    existing = destination / 'tone_vocals.wav'
    existing.write_bytes(b'preserve this existing file')
    session.click(480, 308)
    session.observe('save-dialog', ['Save all separated stems', 'Destination folder'])
    private.run_checked([str(session.pointer), 'drag', '1280', '800', '280', '130', '640', '350'], env=session.env)
    session.observe('save-dialog-positioned', ['Save all separated stems', 'Destination folder'])
    x, y = session.word('save-dialog-positioned', 'Destination', minimum_y=300)
    session.click(x + 100, y + 45, delay=0.5)
    session.click(x + 100, y + 45, delay=0.5)
    session.observe('destination-focused', ['Destination folder'])
    session.key('a', 'ctrl')
    private.run_checked(['wtype', '-d', '15', str(destination)], env=session.env)
    session.observe('destination-entered', ['Save all separated stems', 'exports'])
    session.click(x + 70, y + 95)  # Verified Save all stems button below the positioned field.
    session.observe('partial-save', ['Saved 3 file(s)', 'failed:'])
    assert existing.read_bytes() == b'preserve this existing file'
    for name in ['drums', 'bass', 'other']:
        assert (destination / f'tone_{name}.wav').read_bytes() == expected[name]
    assert len(list(destination.iterdir())) == 4
    denied = base.payload_accesses(base.chunks(session, offsets), replacement)
    assert not denied, denied
    assert not index.exists()
    session.key('Escape')
    session.click(160, 118)
    session.observe('returned-browser', ['Folders', 'tone.wav'])
    base.wait_for(lambda: not job.exists(), 'export completion releases owned stem files')
    (session.output / 'stem-save.json').write_text(json.dumps({
        'completed_stems': 3, 'existing_destination_preserved': True,
        'outputs_match_owned_stems': True, 'replacement_payload_accesses': len(denied),
        'owned_outputs_cleaned': True, 'no_index_created': True,
    }, indent=2))


if __name__ == '__main__':
    raise SystemExit(base.main(exercise,
        'Owned stems export after original replacement; partial completion, no overwrite and no replacement payload reads',
        prepare_environment=lambda env, children: stems.environment(Path(env['XDG_RUNTIME_DIR']).parent, env, children)))
