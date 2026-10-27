#!/usr/bin/env python3
"""Private multi-stem editor copies, rejected replacements and retained lifetimes."""
import json
import os
from pathlib import Path
import shutil
import sys
import file_manager_audio_studio_source_acceptance as base
import file_manager_stem_source_acceptance as stems


def launches(session):
    report = session.output / 'stem-editor.jsonl'
    return [json.loads(line) for line in report.read_text().splitlines()] if report.exists() else []


def exercise(session, root):
    index, job = stems.ready_stems(session, root)
    replacement, offsets = base.replace_audio_root(session, root)
    session.click(1115, 188)
    base.wait_for(lambda: len(launches(session)) == 1, 'editor reads four independent session copies')
    session.observe('stem-editor-copies', ['4 session copies'])
    record = launches(session)[0]
    assert record['names'] == ['vocals.wav', 'drums.wav', 'bass.wav', 'other.wav']
    assert len(set(record['copies'])) == 4
    assert all(record['waves'])
    assert all(Path(path).stat().st_mode & 0o077 == 0 for path in record['copies'])
    assert Path(record['profile']).stat().st_mode & 0o077 == 0
    assert all(not Path(path).is_relative_to(job) for path in record['copies'])
    vocals = job / 'vocals.wav'
    vocals.rename(job / 'vocals-original.wav')
    vocals.symlink_to(replacement)
    session.click(1115, 188)
    session.observe('rejected-stem-editor', ['Could not open audio editor', 'Separated stem changed'])
    assert len(launches(session)) == 1
    denied = base.payload_accesses(base.chunks(session, offsets), replacement)
    assert not denied, denied
    assert not index.exists()
    session.click(160, 118)
    session.observe('returned-browser', ['Folders', 'tone.wav'])
    base.wait_for(lambda: not job.exists(), 'original stem files clean after viewer closes')
    for path in record['copies']:
        copy = Path(path)
        assert copy.exists() and copy.stat().st_size > 1000
    assert Path(record['input']).exists(), 'forwarded editor must retain its LOF and aliases'
    # Reclaim only the exact files reported by this run's owned recording editor.
    shutil.rmtree(record['profile'])
    for path in record['copies']:
        Path(path).unlink()
    (session.output / 'stem-editor-source.json').write_text(json.dumps({
        'copies':4, 'named_tracks':True, 'copies_and_profile_private':True,
        'original_stems_cleaned':True, 'copies_survive_viewer_close':True,
        'replacement_rejected':True, 'replacement_payload_accesses':len(denied),
        'no_index_created':True, 'model_quality_evaluated':False,
    }, indent=2))


def environment(output, env, children):
    audio = stems.environment(output, env, children)
    tools = output / 'tools'
    tools.mkdir()
    editor = tools / 'audacity'
    editor.write_text('#!' + sys.executable + '\n' +
        'import json, os, shlex, sys\nfrom pathlib import Path\n'
        'lof = Path(sys.argv[1])\n'
        'aliases = [Path(shlex.split(line)[1]) for line in lof.read_text().splitlines()]\n'
        'copies = [path.resolve(strict=True) for path in aliases]\n'
        'headers = [path.read_bytes()[:12] for path in copies]\n'
        'record = {"input":str(lof), "profile":str(lof.parent), "names":[path.name for path in aliases], '
        '"copies":[str(path) for path in copies], "waves":[header[:4] == b"RIFF" and header[8:] == b"WAVE" for header in headers]}\n'
        'with open(os.environ["CLIP_STEM_EDITOR_REPORT"], "a") as report: report.write(json.dumps(record) + "\\n")\n')
    editor.chmod(0o700)
    env['PATH'] = str(tools) + os.pathsep + env['PATH']
    env['CLIP_STEM_EDITOR_REPORT'] = str(output / 'stem-editor.jsonl')
    return audio


if __name__ == '__main__':
    raise SystemExit(base.main(exercise,
        'Guarded multi-stem editor handoff, private independent copies and zero replacement reads',
        prepare_environment=lambda env, children: environment(Path(env['XDG_RUNTIME_DIR']).parent, env, children)))
