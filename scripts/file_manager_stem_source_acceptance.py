#!/usr/bin/env python3
"""Guarded stem preparation with real WAV I/O and a download-free tensor fixture."""
import json
import os
from pathlib import Path
import sys
import file_manager_audio_studio_source_acceptance as base

REAL_PYTHON = Path.home() / '.local/share/little-screen-clip/nobg-venv/bin/python'
FIXTURE = Path(__file__).resolve().parent / 'stem_worker_fixture.py'


def ready_stems(session, root):
    index = base.open_checked_audio(session, root)
    session.key('s')
    session.observe('stems-ready', ['four temporary stereo stems', 'All stems'], timeout_seconds=60)
    report = session.output / 'stem-model.jsonl'
    assert len(report.read_text().splitlines()) == 1
    cache = Path(session.env.get('XDG_CACHE_HOME', str(Path(session.env['HOME']) / '.cache')))
    jobs = list((cache / 'little-screen-clip/artifacts/stems').glob('*/stem-job-*'))
    assert len(jobs) == 1 and jobs[0].stat().st_mode & 0o077 == 0
    assert len(list(jobs[0].glob('*.wav'))) == 4
    return index, jobs[0]


def exercise(session, root):
    index, job = ready_stems(session, root)
    report = session.output / 'stem-model.jsonl'
    replacement, offsets = base.replace_audio_root(session, root)
    previous_size = base.audio_tail(session)[1]
    play = (280, 370)  # Verified group Play All control in the private 1280x800 studio.
    session.click(*play)
    def heard():
        values, size = base.audio_tail(session)
        return size > previous_size + 32000 and any(abs(value) > 100 for value in values)
    base.wait_for(heard, 'owned separated stems play after original-root replacement')
    session.click(*play)
    session.observe('owned-stems-paused', ['four temporary stereo stems'])
    # Replace one owned stem after successful playback. A failed decoder must
    # not publish any of the remaining tracks or consume the symlink target.
    vocals = job / 'vocals.wav'
    vocals.rename(job / 'vocals-original.wav')
    vocals.symlink_to(replacement)
    session.click(*play)
    session.observe('rejected-output-replacement', ['Could not play stems', 'Separated stem changed'])
    _, rejected_size = base.audio_tail(session)
    def silent():
        values, size = base.audio_tail(session)
        return size > rejected_size + 64000 and all(abs(value) <= 100 for value in values)
    base.wait_for(silent, 'failed group admission publishes no partial audio')
    session.click(325, 308)  # Verified Separate Again control.
    session.observe('rejected-stem-retry', ['Search result changed'])
    assert len(report.read_text().splitlines()) == 1
    denied = base.payload_accesses(base.chunks(session, offsets), replacement)
    assert not denied, denied
    assert not index.exists()
    session.click(160, 118)
    session.observe('returned-browser', ['Folders', 'tone.wav'])
    base.wait_for(lambda: not job.exists(), 'unexported stem job cleans after the final viewer/read owner')
    (session.output / 'stem-source.json').write_text(json.dumps({
        'prepared_tensor_runs':1, 'real_wav_output_count':4, 'private_output_directory':True,
        'owned_stems_play_after_original_replacement':True, 'retry_visibly_rejected':True,
        'replacement_payload_accesses':len(denied), 'owned_outputs_cleaned':True, 'no_index_created':True,
        'model_quality_evaluated':False,
        'output_replacement_rejected':True, 'no_partial_group_audio':True,
    }, indent=2))


def environment(output, env, children):
    assert REAL_PYTHON.exists()
    python = Path(env['HOME']) / '.local/share/little-screen-clip/nobg-venv/bin/python'
    python.parent.mkdir(parents=True)
    python.write_text('#!' + sys.executable + '\nimport os, sys\nassert sys.argv[1] == "-I"\n' +
        'os.execv(' + repr(str(REAL_PYTHON)) + ', [' + repr(str(REAL_PYTHON)) +
        ', "-I", ' + repr(str(FIXTURE)) + ', sys.argv[0], *sys.argv[2:]])\n')
    python.chmod(0o700)
    env['CLIP_STEM_FIXTURE_REPORT'] = str(output / 'stem-model.jsonl')
    return base.start_private_audio(output, env, children)


if __name__ == '__main__':
    raise SystemExit(base.main(exercise,
        'Checked stem preparation, owned output playback and zero replacement payload reads; tensor fixture without model inference',
        prepare_environment=lambda env, children: environment(Path(env['XDG_RUNTIME_DIR']).parent, env, children)))
