#!/usr/bin/env python3
"""Embedded video, clocked hover playback and owned cleanup on private Wayland/audio."""
from pathlib import Path
import json
import os
import subprocess
import time
from PIL import Image, ImageChops
from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance, wait_for, preserve_file_clipboard
from file_manager_phase4_acceptance import click_word
from file_manager_media_audio import start_private_audio, audio_tail
import ui_acceptance_capture as private


def windows(session):
    return json.loads(private.run_checked([str(session.niri), 'msg', '--json', 'windows'], env=session.env))


def players(session):
    result = subprocess.run(['pgrep', '-P', str(session.process.pid), '-a'], capture_output=True, text=True)
    assert 'ffplay' not in result.stdout.lower(), result.stdout
    return [int(line.split()[0]) for line in result.stdout.splitlines() if 'clip-media-worker playback ' in line]


def capture(session, name):
    path = session.output / (name + '.png')
    path.unlink(missing_ok=True)
    with preserve_file_clipboard(session.env):
        private.screenshot(session.niri, session.env, path)
    return Image.open(path).convert('RGB')


def changed(first, second):
    return sum(pixel != (0, 0, 0) for pixel in ImageChops.difference(first, second).get_flattened_data())


def exercise_full(session, root):
    filename = next(root.glob('movie.*')).name
    session.observe('video-folder', [filename])
    x, y = session.word('video-folder', filename, minimum_y=200)
    session.click(x, y)
    session.key('Return')
    session.observe('embedded-player', [filename, 'Back to folder'])
    owned = players(session)
    assert len(owned) == 1, owned
    assert len([w for w in windows(session) if w['app_id'] == 'little-clip-file-manager']) == 1
    assert not any(w['title'] == 'Files video' for w in windows(session))
    wait_for(lambda: any(abs(value)>500 for value in audio_tail(session)[0][-2000:]), 'embedded playback supplies private audio')
    first = capture(session, 'playing-first')
    time.sleep(0.4)
    assert changed(first, capture(session, 'playing-second')) > 1000
    session.key('space')
    session.observe('paused-player', ['Back to folder'])
    time.sleep(0.2)
    wait_for(lambda: audio_tail(session)[0] and all(abs(value)<50 for value in audio_tail(session)[0][-2000:]), 'pausing video also pauses its audio')
    first = capture(session, 'paused-first')
    time.sleep(0.4)
    assert changed(first.crop((350, 290, 1050, 600)), capture(session, 'paused-second').crop((350, 290, 1050, 600))) < 100
    session.key('Right')
    time.sleep(0.2)
    assert changed(first, capture(session, 'paused-seek')) > 1000
    session.key('space')
    session.observe('resumed-player', [filename, 'Back to folder'])
    session.click(1120, 72)  # Back to folder in the observed fixed-output main bar.
    wait_for(lambda: all(not Path(f'/proc/{pid}').exists() for pid in owned), 'return to folder reaps playback')
    session.observe('returned-folder', [filename])
    x, y = session.word('returned-folder', filename, minimum_y=200)
    session.click(x, y)
    session.key('Return')
    session.observe('reopened-player', [filename, 'Back to folder'])
    session.required_closed_processes = players(session)
    assert len(session.required_closed_processes) == 1


def move(session, x, y):
    pointer = subprocess.Popen([str(session.pointer), 'move', '1280', '800', str(x), str(y)], env=session.env, stdout=subprocess.DEVNULL)
    time.sleep(0.15)
    pointer.terminate()
    pointer.wait(timeout=3)


def exercise_hover(session, root):
    filename = next(root.glob('movie.*')).name
    session.observe('hover-folder', [filename, 'Hover to play'])
    x, y = session.word('hover-folder', filename, minimum_y=200)
    # The filename is the start of the video row; its poster begins 32 points
    # below the row top. Keep the pointer inside the source poster, then move
    # into the separate timeline area on the same row.
    move(session, x+35, y+55)
    session.observe('thumbnail-hover', ['From', '0.00'], preserve_pointer=True)
    owned = players(session)
    assert len(owned)==1, owned
    def ready():
        shot = capture(session, 'hover-ready')
        pixels = shot.crop((x, y+10, min(x+340, 1279), min(y+260, 799))).get_flattened_data()
        return sum(max(pixel)-min(pixel)>70 for pixel in pixels)>15000
    wait_for(ready, 'hover decodes and paints its video frame')
    first = capture(session, 'hover-playing-first')
    time.sleep(0.35)
    assert changed(first, capture(session, 'hover-playing-second'))>1000
    assert all(abs(sample)<50 for sample in audio_tail(session)[0][-4000:]), 'hover must remain silent'
    move(session, 120, 40)
    wait_for(lambda: all(not Path(f'/proc/{pid}').exists() for pid in owned), 'leaving hover reaps player')
    move(session, min(x+420, 1000), y+55)
    session.observe('timeline-hover', ['From'], preserve_pointer=True)
    owned = players(session)
    assert len(owned)==1, owned
    time.sleep(0.4)
    first = capture(session, 'timeline-playing-first')
    time.sleep(0.35)
    assert changed(first, capture(session, 'timeline-playing-second'))>1000
    move(session, min(x+520, 1100), y+55)
    time.sleep(0.25)
    assert players(session)==owned, 'moving across a timeline should reuse its decoder'
    move(session, 120, 40)
    wait_for(lambda: all(not Path(f'/proc/{pid}').exists() for pid in owned), 'timeline hover retires after leaving')
    starts = [json.loads(line) for line in (session.output/'playback-starts.jsonl').read_text().splitlines()]
    assert starts[0]['start_ms']==0, starts
    assert starts[-1]['start_ms']>1000, starts


def exercise(session, root):
    exercise_hover(session, root)
    exercise_full(session, root)


def main():
    repo = Path(__file__).resolve().parent.parent
    args = application_arguments(__doc__, repo / 'target/debug/clip-file-manager')
    output, root = media_fixture_directory(args)
    private.run_checked(['ffmpeg', '-v', 'error', '-f', 'lavfi', '-i', 'testsrc2=size=320x180:rate=24:duration=60', '-f', 'lavfi', '-i', 'sine=frequency=440:duration=60', '-c:v', 'libx264', '-threads', '2', '-preset', 'ultrafast', '-c:a', 'aac', '-shortest', str(root/'movie.mp4')], env=os.environ, timeout=30)
    wrapper = output/'playback-worker'
    wrapper.write_text("#!/usr/bin/env python3\nimport os,sys,json\nif sys.argv[1]=='playback':\n with open(os.environ['PLAYBACK_STARTS'],'a') as log: log.write(json.dumps(json.loads(sys.argv[2]))+'\\n')\nos.execv(os.environ['REAL_MEDIA_WORKER'],['clip-media-worker',*sys.argv[1:]])\n")
    wrapper.chmod(0o700)
    def environment(env, children):
        start_private_audio(output, env, children)
        env.update(LITTLE_CLIP_MEDIA_WORKER=str(wrapper), REAL_MEDIA_WORKER=str(args.binary.resolve().with_name('clip-media-worker')), PLAYBACK_STARTS=str(output/'playback-starts.jsonl'))
    return run_application_acceptance(args, output, root, exercise, 'embedded-and-hover-video', environment)


if __name__ == '__main__':
    raise SystemExit(main())
