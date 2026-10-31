#!/usr/bin/env python3
"""Saving preferences interrupts a held answer and retains completed session cells."""
import hashlib
import json
from pathlib import Path
import subprocess
import time
import os
from PIL import Image, ImageOps
import ui_acceptance_capture as private

from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance, wait_for
from file_manager_frontmatter_acceptance import environment
from file_manager_search_acceptance import pause_indexing, submit


CANARY = 'ANSWER_PREEMPT_CANARY_3821'


def exercise(session, root, binary):
    question = 'What numeric score is explicitly listed?'
    before = {p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in root.glob('*.txt')}
    session.observe('initial', ['Alpha.txt', 'Beta.txt'])
    session.click(647, 160)
    session.click(*session.word('initial', 'Alpha.txt', minimum_y=250))
    submit(session, ':q:"'+question+'";type:number')
    marker, release = session.output/'held-answer.pid', session.output/'release-answer'
    try:
        wait_for(marker.exists, 'second file reaches its foreground answer gate', 60)
        pid = int(marker.read_text())
        assert Path(f'/proc/{pid}').exists() and not release.exists()
        session.observe('completed-cell-before-interruption', ['Alpha.txt', 'Beta.txt', '10 points', 'Session answers'])
        started = time.monotonic()
        try:
            pause_indexing(session, True, 'save-during-answer')
        finally:
            (session.output/'preemption.json').write_text(json.dumps(dict(
                held_pid=pid, child_reaped=not Path(f'/proc/{pid}').exists(),
                gate_released=release.exists(), control_seconds=time.monotonic()-started), indent=2))
        assert not Path(f'/proc/{pid}').exists() and not release.exists()
        session.observe('interrupted-answer-retried', ['Alpha.txt', 'Beta.txt', '10 points', '2 points', 'Session answers'], timeout_seconds=60)
        assert before == {p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in root.glob('*.txt')}
        index = Path(session.env['XDG_DATA_HOME'])/'little-screen-clip/search/index.sqlite'
        assert not index.exists(), 'Q/A and saving preferences must not implicitly enroll'
        session.address(root/'Nested')
        session.observe('other-folder', ['This folder is empty'])
        session.address(root)
        session.observe('return-retains-completed-cells', ['10 points', '2 points', 'Session answers'])
        assert not release.exists(), 'retry completes without releasing the abandoned helper'
        session.close()
        session.restart(binary, root)
        session.key('F24')
        text = session.observe('restart-discards-session-columns', ['Alpha.txt', 'Beta.txt'])
        assert 'session answers' not in text.lower()
        assert not index.exists()
        artifacts = [session.output/'files.log', session.output/'files-resumed.log', session.output/'scratch/config.toml']
        for directory, _, files in os.walk(index.parent, followlinks=False):
            artifacts.extend(Path(directory)/name for name in files if not (Path(directory)/name).is_symlink())
        for artifact in artifacts:
            data = artifact.read_bytes()
            for secret in [question, CANARY, '10 points', '2 points']:
                assert secret.encode() not in data, ('session content persisted', artifact, secret)
        (session.output/'session-persistence.json').write_text(json.dumps(dict(
            restart_has_no_columns=True, index_created=False,
            checked_artifacts=[str(path) for path in artifacts],
            question_answer_canary_absent=True), indent=2))
    finally:
        release.touch()



def exercise_freshness(session, root, _binary):
    beta = root / 'Beta.txt'
    alpha = root / 'Alpha.txt'
    original_beta = beta.read_text()
    original_alpha = alpha.read_text()
    original = {path.name:path.stat() for path in (alpha, beta)}
    marker, release = session.output/'held-answer.pid', session.output/'release-answer'
    checks = {}
    try:
        session.observe('initial', ['Alpha.txt', 'Beta.txt'])
        session.click(647, 160)
        submit(session, ':q:"What numeric score is explicitly listed?";type:number')
        wait_for(marker.exists, 'held answer before source edit', 60)
        pid = int(marker.read_text())
        assert Path(f'/proc/{pid}').exists() and not release.exists()
        session.observe('before-source-edit', ['10 points', 'Session answers'])
        beta.write_text(original_beta.replace('Score: 2', 'Score: 7'))
        os.utime(beta, ns=(original['Beta.txt'].st_atime_ns, original['Beta.txt'].st_mtime_ns))
        changed = beta.stat()
        assert (changed.st_ino, changed.st_size, changed.st_mtime_ns) == (
            original['Beta.txt'].st_ino, original['Beta.txt'].st_size, original['Beta.txt'].st_mtime_ns)
        assert changed.st_ctime_ns != original['Beta.txt'].st_ctime_ns
        checks['in_place_preserved_inode_size_mtime'] = True
        release.touch()
        session.observe('edited-source-answer', ['10 points', '7 points', 'Session answers'], timeout_seconds=90)
        replacement = root / 'replacement.tmp'
        replacement.write_text(original_alpha.replace('Score: 10', 'Score: 11'))
        os.utime(replacement, ns=(original['Alpha.txt'].st_atime_ns, original['Alpha.txt'].st_mtime_ns))
        replacement.replace(alpha)
        changed = alpha.stat()
        assert changed.st_ino != original['Alpha.txt'].st_ino
        assert (changed.st_size, changed.st_mtime_ns) == (original['Alpha.txt'].st_size, original['Alpha.txt'].st_mtime_ns)
        session.observe('replaced-source-answer', ['11 points', '7 points', 'Session answers'], timeout_seconds=90)
        checks['replacement_preserved_size_mtime_new_inode'] = True
        session.address(root/'Nested')
        session.observe('away', ['This folder is empty'])
        previous = alpha.stat()
        alpha.write_text(original_alpha.replace('Score: 10', 'Score: 13'))
        os.utime(alpha, ns=(previous.st_atime_ns, previous.st_mtime_ns))
        session.address(root)
        session.observe('return-revalidates-answer', ['13 points', '7 points', 'Session answers'], timeout_seconds=90)
        checks['folder_return_revalidates_completed_cell'] = True
        assert alpha.read_text() == original_alpha.replace('Score: 10', 'Score: 13')
        assert beta.read_text() == original_beta.replace('Score: 2', 'Score: 7')
        index = Path(session.env['XDG_DATA_HOME'])/'little-screen-clip/search/index.sqlite'
        assert not index.exists()
        checks['no_implicit_index'] = True
        checks['held_pid'] = pid
        checks['scope'] = 'Final answers after controlled edits/replacement/return; not continuous frame-by-frame nonpublication or remote/remount coverage.'
        (session.output/'freshness-checks.json').write_text(json.dumps(checks, indent=2))
    finally:
        release.touch()


def exercise_close(session, root, _binary):
    marker, release = session.output/'held-answer.pid', session.output/'release-answer'
    source = session.output/'input'/'payload.bin'
    source.parent.mkdir()
    source.write_bytes(b'Accepted copy must survive the first close request.\n')
    destination = root/'payload.bin'
    original = {p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in root.glob('*.txt')}
    try:
        session.observe('initial', ['Alpha.txt', 'Beta.txt', 'payload.bin'])
        session.click(647, 160)
        submit(session, ':q:"What numeric score is explicitly listed?";type:number')
        wait_for(marker.exists, 'answer request held before close', 60)
        pid = int(marker.read_text())
        assert Path(f'/proc/{pid}').exists() and not release.exists()
        session.key('Escape')
        session.observe('answer-held', ['Alpha.txt', 'Beta.txt', '10 points', 'Pending', 'Session answers'])
        session.click(*session.word('answer-held', 'Alpha.txt', minimum_y=280))
        subprocess.run(['wl-copy', '--type', 'x-special/gnome-copied-files'],
                       input=('copy\n'+source.as_uri()+'\n').encode(), env=session.env,
                       check=True, timeout=5, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        session.key('v', 'ctrl')
        session.observe('accepted-copy-waits', ['Destination already exists', 'payload.bin', 'Replace', 'Skip'])
        session.key('q', 'ctrl')
        session.observe('close-deferred-for-copy', ['Destination already exists', 'Replace', 'Skip'])
        # The status is dimmed behind the modal. A crop prevents foreground
        # dialog text from hiding this separate close-deferral observation.
        status = session.output/'close-deferral-status.png'
        with Image.open(session.output/'close-deferred-for-copy.png') as shot:
            ImageOps.autocontrast(shot.crop((95, 700, 1190, 737)).convert('L')).resize((3285, 111)).save(status)
        text = private.run_checked(['tesseract', str(status), 'stdout', '--psm', '6'], env=session.env)
        (session.output/'close-deferral-status.txt').write_text(text)
        assert 'waitforthefileoperationtofinishbeforeclosing' in ''.join(text.lower().split()), text
        assert session.process.poll() is None
        assert destination.read_bytes() == b'Original destination.\n'
        assert Path(f'/proc/{pid}').exists() and not release.exists()
        marker.unlink()
        # The private runner fixes 1280x800 and these owned paths give a fixed
        # dialog layout. Its Replace button is below the explanatory mentions.
        session.click(430, 500)
        wait_for(lambda: destination.read_bytes() == source.read_bytes(), 'accepted copy installs exact bytes', 10)
        # Transfer completion refreshes the directory, correctly cancelling the
        # old read. Hold its replacement so final close still has pending work.
        wait_for(marker.exists, 'refreshed pending answer reaches replacement gate', 60)
        closing_pid = int(marker.read_text())
        assert closing_pid != pid and not Path(f'/proc/{pid}').exists()
        session.observe('copy-completed-with-answer-held', ['Alpha.txt', 'Beta.txt', 'payload.bin', 'Session answers', 'Pending'])
        assert Path(f'/proc/{closing_pid}').exists() and not release.exists()
        started = time.monotonic()
        session.close()
        assert not Path(f'/proc/{closing_pid}').exists(), 'Normal close must kill and reap the held answer helper'
        assert not release.exists(), 'Close must not depend on releasing the test gate'
        assert destination.read_bytes() == source.read_bytes()
        assert original == {p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in root.glob('*.txt')}
        index = Path(session.env['XDG_DATA_HOME'])/'little-screen-clip/search/index.sqlite'
        assert not index.exists()
        (session.output/'close-and-copy.json').write_text(json.dumps(dict(
            original_held_pid=pid, closing_held_pid=closing_pid,
            refresh_reaped_original=True, close_seconds=time.monotonic()-started, child_reaped=True,
            first_close_deferred=True, accepted_copy_completed=True,
            destination_sha256=hashlib.sha256(destination.read_bytes()).hexdigest(),
            original_answer_sources_unchanged=True, gate_released=False, implicit_index=False,
            scope='Normal close with an owned answer request held before delivery; accepted copy waits for a conflict decision, survives first close and completes before final close. Not an interrupted mid-installation or uninterruptible-I/O test.'), indent=2))
    finally:
        release.touch()


def main():
    repo = Path(__file__).resolve().parent.parent
    def configure(parser):
        mode = parser.add_mutually_exclusive_group()
        mode.add_argument('--source-freshness', action='store_true', help='Edit/replace sources with preserved sizes and mtimes around a held answer and folder return')
        mode.add_argument('--close-while-pending', action='store_true', help='Defer close for an accepted copy, then close and reap the held answer helper')
    args = application_arguments(__doc__, repo/'target/release/clip-file-manager', configure)
    output, root = media_fixture_directory(args)
    (root/'Alpha.txt').write_text('Test record. Score: 10 points. Result: failed.')
    (root/'Beta.txt').write_text(CANARY+'\nTest record. Score: 2 points. Result: passed.')
    (root/'Nested').mkdir()
    if args.close_while_pending:
        (root/'payload.bin').write_bytes(b'Original destination.\n')
    library = output/'answer-stall.so'
    subprocess.run(['cc', '-shared', '-fPIC', '-O2', '-Wall', '-Wextra', '-Werror', str(repo/'scripts/fixtures/search_reader_stall.c'), '-ldl', '-o', str(library)], check=True)
    def prepare(env, children):
        environment(env, children)
        env.update(LD_PRELOAD=str(library), CLIP_TEST_STALL_MODEL_TEXT=CANARY,
                   CLIP_TEST_STALL_MODEL_ROLE='answer', CLIP_TEST_STALL_MODEL_OP='*',
                   CLIP_TEST_STALL_MARKER=str(output/'held-answer.pid'), CLIP_TEST_STALL_RELEASE=str(output/'release-answer'))
    scenario = exercise_close if args.close_while_pending else exercise_freshness if args.source_freshness else exercise
    scope = ('Real Q8 held answer: accepted copy survives close request, completes exact bytes, then normal close kills/reaps the answer helper without releasing its gate' if args.close_while_pending else
             'Real Q8 held answer with in-place edits, same-path replacement and folder return; preserved size/mtime, current typed values, no implicit index' if args.source_freshness else
             'Real Q8 answer request held before delivery: preferences save cancels/reaps helper, interrupted cell retries and completed values survive folder return; restart discards columns, no question/value logging, index or source writes')
    return run_application_acceptance(args, output, root, lambda session, root: scenario(session, root, args.binary), scope, prepare)


if __name__ == '__main__':
    raise SystemExit(main())
