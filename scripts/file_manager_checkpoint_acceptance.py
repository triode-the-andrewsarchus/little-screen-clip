#!/usr/bin/env python3
"""Close/reopen a genuinely unfinished indexed document in a private Files session."""
import hashlib
import json
import math
from pathlib import Path
import re
import sqlite3
import struct
import subprocess
import sys
import time
from array import array

from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance, wait_for
from file_manager_maintenance_acceptance import search_environment
from file_manager_real_corpus_acceptance import wait_index
from file_manager_search_acceptance import submit
import ui_acceptance_capture as private


def verify_segments(rows, source):
    """This fixture is ASCII/LF plain text: extracted offsets are source offsets."""
    covered = 0
    for _, location, input_hash, _ in rows:
        match = re.search(r'UTF-8 bytes (\d+)\.\.(\d+)$', location)
        assert match, location
        start, end = map(int, match.groups())
        assert 0 <= start <= covered < end <= len(source), (start,end,covered)
        assert hashlib.sha256(b'title: none | text: '+source[start:end]).digest() == input_hash
        covered = end
    assert covered == len(source), (covered,len(source))
    return {'source_bytes':len(source), 'covered_bytes':covered, 'verified_segments':len(rows)}


def file_hash(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def audio_fixture(path):
    """Make a 425s PCM WAV with sparse padding above the full-digest limit."""
    data_size = 425 * 32000
    padding = 60 * 1024 * 1024
    total = 44 + data_size + 8 + padding
    header = struct.pack('<4sI4s4sIHHIIHH4sI', b'RIFF', total - 8, b'WAVE',
        b'fmt ', 16, 1, 1, 16000, 32000, 2, 16, b'data', data_size)
    with path.open('wb') as stream:
        stream.write(header)
        for start in range(0, 425, 25):
            frequency = 180 + start
            samples = array('h', (int(12000 * math.sin(2 * math.pi * frequency * i / 16000))
                                  for i in range(16000)))
            if sys.byteorder != 'little':
                samples.byteswap()
            stream.seek(44 + start * 32000)
            samples.tofile(stream)
        stream.seek(44 + data_size)
        stream.write(struct.pack('<4sI', b'JUNK', padding))
        stream.truncate(total)


def exercise_media_clock(session, root, binary):
    session.observe('clock-initial', ['Audio.mkv', 'Video.mkv'])
    sources = {p.name:file_hash(p) for p in root.iterdir()}
    index = Path(session.env['XDG_DATA_HOME'])/'little-screen-clip/search/index.sqlite'
    assert not index.exists()
    session.click(993, 118)
    session.key('Escape')
    from file_manager_reuse_acceptance import inventory
    generation, paths, count = inventory(index)
    assert len(paths) == 2 and count == 18, (paths,count)
    with sqlite3.connect(index) as db:
        for name, visual in [('Audio.mkv',False),('Video.mkv',True)]:
            rows = db.execute('SELECT s.modality,s.location,f.coverage FROM segments s JOIN files f ON f.id=s.file_id JOIN paths p ON p.file_id=f.id WHERE p.path=? ORDER BY s.id', [str(root/name).encode()]).fetchall()
            assert [loc for kind,loc,_ in rows if kind=='audio'] == ['audio seconds 0..30','audio seconds 25..55','audio seconds 50..60'], rows
            assert len(rows) == (15 if visual else 3), rows
            assert {coverage for _,_,coverage in rows} == {'sampled: visual frames; selected audio track' if visual else 'complete'}, rows
        first = db.execute("SELECT p.path,s.vector FROM segments s JOIN paths p ON p.file_id=s.file_id WHERE s.modality='audio' AND s.location='audio seconds 0..30' ORDER BY p.path").fetchall()
    session.click(993, 118)
    session.observe('clock-complete', ['2 files','18 vectors'])
    session.key('Escape')
    session.close()
    with sqlite3.connect(index) as db:
        # A prior representation missing later windows; keep the compatible
        # canonical first window and frames to verify exact-input reuse.
        db.execute("DELETE FROM segments WHERE modality='audio' AND location<>'audio seconds 0..30'")
        for file, representation in db.execute('SELECT id,format FROM files').fetchall():
            suffix = b'\0media-timeline-v2'
            assert representation.endswith(suffix)
            db.execute('UPDATE files SET format=? WHERE id=?', [representation[:-len(suffix)],file])
            db.execute('UPDATE source_digests SET format=? WHERE file_id=?', [representation[:-len(suffix)],file])
    session.restart(binary, root)
    session.key('F24')
    resumed, paths, count = inventory(index,generation)
    assert resumed > generation and len(paths) == 2 and count == 18
    with sqlite3.connect(index) as db:
        after = db.execute("SELECT p.path,s.vector FROM segments s JOIN paths p ON p.file_id=s.file_id WHERE s.modality='audio' AND s.location='audio seconds 0..30' ORDER BY p.path").fetchall()
        assert after == first
    session.click(993,118)
    session.observe('clock-policy-repaired', ['2 files','18 vectors','2 generated','16 reused'])
    assert sources == {p.name:file_hash(p) for p in root.iterdir()}
    (session.output/'media-clock-checks.json').write_text(json.dumps(dict(
        source_sha256=sources, first_window_vectors={bytes(path).decode():hashlib.sha256(vector).hexdigest() for path,vector in after}, initial_generation=generation,
        resumed_generation=resumed, complete_audio_windows=6, visual_samples=12,
        audio_only_coverage_is_complete=True, unchanged_sources=True,
        scope='Gapped audio-only/video containers and simulated prior incomplete catalog; original collapsed PCM differs separately in reader before/after evidence.'),indent=2))


def verify_audio_segments(rows, source):
    """Compare every window and canonical WAV input with this owned PCM fixture."""
    covered = 0
    with source.open('rb') as stream:
        for index, (_, location, input_hash, vector) in enumerate(rows):
            match = re.fullmatch(r'audio seconds (\d+(?:\.\d+)?)\.\.(\d+(?:\.\d+)?)', location)
            assert match, location
            start, end = map(float, match.groups())
            assert start == index * 25 and start <= covered < end <= 425, (start, end, covered)
            stream.seek(44 + int(start * 32000))
            pcm = stream.read(int((end - start) * 32000))
            wav = struct.pack('<4sI4s4sIHHIIHH4sI', b'RIFF', 36 + len(pcm), b'WAVE',
                b'fmt ', 16, 1, 1, 16000, 32000, 2, 16, b'data', len(pcm)) + pcm
            assert hashlib.sha256(wav).digest() == input_hash, location
            assert vector is not None
            covered = end
    assert covered == 425, covered
    return {'duration_seconds':425, 'covered_seconds':covered, 'verified_windows':len(rows)}


def verify_media_segments(rows, source, mode):
    if mode == 'audio':
        return verify_audio_segments(rows, source)
    if mode == 'text':
        return verify_segments(rows, source.read_bytes())
    assert len(rows) == 17, len(rows)
    for index, (_, location, input_hash, vector) in enumerate(rows):
        timestamp = index * 5 + 2.5
        assert location == f'video frame at seconds {timestamp:.6f}', location
        frame = subprocess.run(['ffmpeg', '-v', 'error', '-nostdin', '-threads', '2',
            '-filter_threads', '1', '-max_alloc', '268435456', '-copyts', '-ss', str(timestamp),
            '-i', str(source), '-map', '0:0', '-an', '-sn', '-dn', '-vf',
            'scale=1024:1024:force_original_aspect_ratio=decrease,showinfo', '-frames:v', '1',
            '-fps_mode', 'passthrough', '-c:v', 'png', '-f', 'image2pipe', 'pipe:1'],
            capture_output=True, check=True, timeout=10).stdout
        assert frame.startswith(b'\x89PNG'), location
        assert hashlib.sha256(b'vision-tokens:140\0' + frame).digest() == input_hash, location
        assert vector is not None
    return {'duration_seconds':85, 'sampling_interval_seconds':5, 'verified_frames':len(rows)}


def checkpoint(index):
    if not index.exists():
        return None
    try:
        with sqlite3.connect(index) as db:
            row = db.execute("SELECT c.file_id,c.progress,c.segments,f.coverage,r.state,f.revision,r.error FROM file_checkpoints c JOIN files f ON f.id=c.file_id JOIN paths p ON p.file_id=f.id JOIN memberships m ON m.path=p.path JOIN roots r ON r.id=m.root_id").fetchone()
            if not row:
                return None
            prefix = db.execute('SELECT id,location,input_hash,vector FROM segments WHERE file_id=? ORDER BY id', [row[0]]).fetchall()
        return {'file':row[0], 'progress':json.loads(row[1]), 'count':row[2], 'coverage':row[3], 'root_state':row[4], 'revision':row[5], 'root_error':row[6], 'prefix':prefix}
    except sqlite3.OperationalError as error:
        if 'no such table' in str(error):
            return None
        raise


def completed_segments(index, saved, replace, mode):
    with sqlite3.connect(index) as db:
        assert db.execute('SELECT COUNT(*) FROM file_checkpoints').fetchone()[0] == 0
        final = db.execute('SELECT id,location,input_hash,vector FROM segments WHERE file_id=? ORDER BY id', [saved['file']]).fetchall()
        if replace:
            assert len(final) == 1
            assert final[0][2] not in {row[2] for row in saved['prefix']}
            assert db.execute('SELECT revision FROM files WHERE id=?',[saved['file']]).fetchone()[0] != saved['revision']
        else:
            assert len(final) > saved['count']
            assert final[:saved['count']] == saved['prefix'], 'Saved locations/IDs/canonical vectors changed'
        assert len({row[1] for row in final}) == len(final), 'Restart duplicated a passage location'
        coverage = db.execute('SELECT coverage FROM files WHERE id=?',[saved['file']]).fetchone()[0]
        assert coverage == ('sampled: visual frames; no audio track observed' if mode == 'video' else 'complete'), coverage
    return final


def close_at_checkpoint(session, index, source, digest, binary, mode):
    # No OCR pause here: stop before this deliberately long file finishes.
    session.key('q', 'ctrl')
    session.process.wait(timeout=10)
    assert session.process.returncode == 0
    saved = checkpoint(index)
    assert saved and saved['count'] >= 8, saved
    assert saved['coverage'].startswith('partial: indexing in progress'), saved
    assert saved['root_state'] == 'building' and saved['root_error'] is None, saved
    assert len(saved['prefix']) == saved['count']
    assert file_hash(source) == digest
    if mode == 'audio':
        assert source.stat().st_size > 64 * 1024 * 1024
        assert saved['progress']['digest'] is None and saved['progress']['prepared_hash'] is None
        assert saved['progress']['text_cursor'] == 0
        assert saved['progress']['cursor'] == saved['count'] * 25
    elif mode == 'video':
        assert saved['progress']['cursor'] == saved['count']
        assert saved['progress']['text_cursor'] == 0
        assert saved['progress']['prepared_hash'] is None
    else:
        assert bytes(saved['progress']['prepared_hash']) == bytes.fromhex(digest)
    binary_hash = file_hash(binary)
    (session.output/'closed-checkpoint.json').write_text(json.dumps({
        **{key:value for key,value in saved.items() if key != 'prefix'},
        'source_sha256':digest, 'binary_sha256':binary_hash,
        'prefix': [{'id':row[0], 'location':row[1],
                    'input_sha256':row[2].hex(), 'vector_sha256':hashlib.sha256(row[3]).hexdigest()}
                   for row in saved['prefix']],
    },indent=2))
    return saved


def observe_completion(session, source, replace, mode, keyboard_close, refresh_virtual_keymap):
    if mode != 'text':
        session.observe(f'{mode}-after-resume', [source.name])
        if keyboard_close:
            windows = private.own_windows(session.niri, session.env, {session.process.pid})
            (session.output/'keyboard-close-window.json').write_text(json.dumps(windows, indent=2))
            assert len(windows) == 1 and windows[0]['is_focused'], windows
            if refresh_virtual_keymap:
                # Controlled diagnostic for a new Wayland client receiving the
                # physical keymap while wtype reuses its previous virtual map.
                # F24 has no application binding and forces a different map.
                session.key('F24')
            session.key('q', 'ctrl')
        else:
            private.run_checked([str(session.niri), 'msg', 'action', 'close-window'], env=session.env)
        session.process.wait(timeout=10)
        assert session.process.returncode == 0
        return ('keyboard-after-virtual-keymap-refresh' if refresh_virtual_keymap else 'keyboard') if keyboard_close else 'window-manager'
    submit(session, ':emb:"checkpoint sentinel quasar"')
    session.observe('old-passage-after-source-change' if replace else 'late-passage-after-resume',
                    ['0 matching entries', 'No matching files'] if replace else ['long.txt', 'Keyword match'])
    return None


def verify_resumed(session, index, source, saved, replace, mode, keyboard_close, refresh_virtual_keymap):
    log = session.output/'files-resumed.log'
    resumed = re.search(r'resumed verified indexing checkpoint.*segments=(\d+)', log.read_text())
    if replace:
        assert not resumed, 'Changed source inherited an old cursor'
    else:
        assert resumed and int(resumed.group(1)) == saved['count']
    final = completed_segments(index,saved,replace,mode)
    coverage = verify_media_segments(final, source, mode)
    if mode != 'text':
        with sqlite3.connect(index) as db:
            modality = 'audio' if mode == 'audio' else 'image'
            assert db.execute('SELECT COUNT(*) FROM segments WHERE file_id=? AND modality=? AND vector IS NOT NULL',
                              [saved['file'], modality]).fetchone()[0] == len(final)
    close_method = observe_completion(session, source, replace, mode, keyboard_close, refresh_virtual_keymap)
    assert file_hash(source) == saved['expected_source_sha256']
    (session.output/'checkpoint-checks.json').write_text(json.dumps({
        'source_sha256':saved['expected_source_sha256'], 'closed_committed_segments':saved['count'],
        'saved_cursor':saved['progress']['cursor'], 'saved_text_cursor':saved['progress']['text_cursor'],
        'source_replaced_while_closed':replace,
        'resumed_segments':int(resumed.group(1)) if resumed else None, 'completed_segments':len(final),
        'canonical_prefix_unchanged':None if replace else True, 'source_unchanged_after_restart':True,
        'no_duplicate_locations':True, 'late_passage_found':None if mode != 'text' else not replace,
        'coverage':coverage, 'final_close_method':close_method,
    },indent=2))


def exercise(session, root, children, binary, replace, mode, keyboard_close, refresh_virtual_keymap):
    source = root/{'audio':'long.wav', 'video':'long.mkv', 'text':'long.txt'}[mode]
    digest = file_hash(source)
    session.observe('initial-unindexed', [source.name])
    index = Path(session.env['XDG_DATA_HOME'])/'little-screen-clip/search/index.sqlite'
    assert not index.exists()
    session.click(993, 118)
    session.key('Escape')
    wait_for(lambda: checkpoint(index), 'first durable partial batch', 300 if mode != 'text' else 120)
    saved = close_at_checkpoint(session, index, source, digest, binary, mode)
    if replace:
        # Only this driver's owned synthetic source; no application is running.
        source.write_text('A replacement document describes river geology and sediment deposits.\n')
        digest = hashlib.sha256(source.read_bytes()).hexdigest()
    saved['expected_source_sha256'] = digest

    session.restart(binary, root)
    log = session.output/'files-resumed.log'
    if not replace:
        wait_for(lambda: 'resumed verified indexing checkpoint' in log.read_text(), 'verified checkpoint resume', 120)
    summary = wait_index(session,index,600 if mode != 'text' else 180,time.monotonic())
    assert summary['roots'] == [('ready',None)], summary
    verify_resumed(session, index, source, saved, replace, mode, keyboard_close, refresh_virtual_keymap)


def main():
    repo = Path(__file__).resolve().parent.parent
    def configure(parser):
        parser.add_argument('--replace-before-resume', action='store_true')
        parser.add_argument('--keyboard-close', action='store_true', help='Verify Ctrl+Q after media checkpoint resume, retaining Wayland input trace')
        parser.add_argument('--refresh-virtual-keymap', action='store_true', help='Diagnostic: send unbound F24 before Ctrl+Q after restart')
        media = parser.add_mutually_exclusive_group()
        media.add_argument('--audio', action='store_true')
        media.add_argument('--video', action='store_true')
        media.add_argument('--media-clock', action='store_true')
    args = application_arguments(__doc__,repo/'target/debug/clip-file-manager',configure)
    mode = 'audio' if args.audio else 'video' if args.video else 'text'
    if args.keyboard_close and mode == 'text':
        raise ValueError('--keyboard-close requires --audio or --video')
    if args.refresh_virtual_keymap and not args.keyboard_close:
        raise ValueError('--refresh-virtual-keymap requires --keyboard-close')
    assert not (mode != 'text' and args.replace_before_resume)
    output, root = media_fixture_directory(args)
    if args.media_clock:
        if any((args.keyboard_close,args.replace_before_resume,args.refresh_virtual_keymap)):
            raise ValueError('--media-clock cannot combine checkpoint-only flags')
        video = root/'Video.mkv'
        subprocess.run(['ffmpeg','-v','error','-f','lavfi','-i','color=c=blue:s=80x40:r=10:d=60',
            '-f','lavfi','-i','sine=frequency=440:sample_rate=16000:duration=60',
            '-af',r'aselect=not(between(t\,10\,12))','-c:v','ffv1','-c:a','pcm_s16le',str(video)],check=True,timeout=30)
        subprocess.run(['ffmpeg','-v','error','-i',str(video),'-vn','-c:a','copy',str(root/'Audio.mkv')],check=True,timeout=30)
        return run_application_acceptance(args,output,root,lambda session,root:exercise_media_clock(session,root,args.binary),
            'Packet-gap audio timeline coverage, selected video frames, truthful audio-only coverage and scoped legacy repair',search_environment)
    paragraph = 'The laboratory records temperature, calibration, materials, observations, and maintenance procedures for this independent experiment. '
    if args.audio:
        audio_fixture(root/'long.wav')
    elif args.video:
        subprocess.run(['ffmpeg', '-v', 'error', '-f', 'lavfi', '-i',
            'testsrc2=size=96x64:rate=2:duration=85', '-an', '-c:v', 'ffv1',
            str(root/'long.mkv')], check=True, timeout=30)
    else:
        with (root/'long.txt').open('w') as stream:
            for index in range(1024):
                stream.write(f'Section {index:04d}. '+paragraph*12+'\n\n')
            stream.write('The final checkpoint sentinel quasar confirms the last passage was indexed.\n')
    state = {}
    def environment(env, children):
        state['children'] = children
        env['LITTLE_CLIP_SEARCH_TIMINGS'] = '1'
        if args.keyboard_close:
            env['WAYLAND_DEBUG'] = 'client'
        search_environment(env, children)
    return run_application_acceptance(args,output,root,
        lambda session,folder:exercise(session,folder,state['children'],args.binary,args.replace_before_resume,mode,args.keyboard_close,args.refresh_virtual_keymap),
        'Audio checkpoint restart with full canonical-window verification' if args.audio else
        'Video checkpoint restart with canonical-frame verification' if args.video else
        'Changed source rejects saved cursor and old passages' if args.replace_before_resume else
        'Explicit enrollment, durable partial batch, real shutdown/restart, verified cursor, canonical prefix, complete late passage',environment)


if __name__ == '__main__':
    raise SystemExit(main())
