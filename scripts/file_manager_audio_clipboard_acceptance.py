#!/usr/bin/env python3
"""Audio Studio copies real file URIs even when the daemon only acknowledges handoff."""
import json
import io
import re
import subprocess
import wave
from pathlib import Path
from urllib.parse import unquote, urlparse
import ui_acceptance_capture as private
from ui_acceptance_fixtures import wav_fixture, midi_fixture
from PIL import Image
from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance, wait_for
from file_manager_phase4_acceptance import open_file
from file_manager_media_audio import start_private_audio
from file_manager_video_playback_acceptance import capture


class AudioDaemon(private.FakeTextPreviewDaemon):
    def __init__(self, runtime, log):
        self.received = []
        self.images = []
        super().__init__(runtime, log)

    def _response(self, request):
        if isinstance(request, dict) and 'PushFile' in request:
            self.received.append(request['PushFile']['path'])
            # Deliberately leave the OS clipboard unchanged after acknowledging.
            return {'Success': {'message': 'Accepted audio'}}, 'audio_push'
        if isinstance(request, dict) and 'PushImage' in request:
            image = request['PushImage']
            self.images.append((bytes(image['bytes']), image['mime']))
            return {'Success': {'message': 'Accepted image'}}, 'image_push'
        return super()._response(request)


def exercise(session, root, daemon):
    open_file(session, root, 'tone.wav', 'audio-row')
    session.observe('audio-studio', ['tone.wav', 'Selection info', 'Copy Track'])
    for index, button in enumerate([True, False]):
        expected_duration = 10.0
        if not button:
            def waveform_ready():
                image = capture(session, 'ready-waveform')
                return sum(g-r>60 and g-b>20 for r,g,b in image.crop((300,475,700,500)).get_flattened_data())>2500
            wait_for(waveform_ready, 'complete stereo waveform before selecting a region')
            private.run_checked([str(session.pointer), 'drag', '1280', '800', '350', '485', '650', '485'], env=session.env)
            selection = session.observe('audio-selection', ['Copy Slice', 'Selection info'])
            match = re.search(r'Duration\s+(\d+):(\d+):(\d+(?:\.\d+)?)', selection)
            assert match, selection
            hours,minutes,seconds = map(float,match.groups())
            expected_duration = hours*3600+minutes*60+seconds
            assert 0<expected_duration<10
        subprocess.run(['wl-copy', '--type', 'text/plain'], input=b'previous unrelated text', env=session.env, check=True, timeout=5)
        if button:
            session.click(530, 297)
        else:
            session.key('c', 'ctrl')
        wait_for(lambda: len(daemon.received)>index, 'audio copy reaches the acknowledging daemon')
        def copied():
            result = subprocess.run(['wl-paste', '--list-types'], env=session.env, capture_output=True, timeout=5)
            return 'text/uri-list' in result.stdout.decode()
        wait_for(copied, 'audio file replaces stale text on the OS clipboard')
        uri = private.run_checked(['wl-paste', '--no-newline', '--type', 'text/uri-list'], env=session.env).strip()
        path = Path(unquote(urlparse(uri).path))
        assert path.is_file() and str(path)==daemon.received[index], uri
        with wave.open(str(path)) as wav:
            duration = wav.getnframes()/wav.getframerate()
            assert wav.getnchannels()==2
            assert abs(duration-expected_duration)<.025, (duration,expected_duration)
        (session.output/f'copied-audio-{index}.json').write_text(json.dumps({'uri':uri,'seconds':duration,'channels':2}))

    for index,(label,height) in enumerate([('Wave',340),('FFT',440)]):
        shot = f'before-png-{index}'
        session.observe(shot,['Copy Wave','Copy FFT'])
        x,y = session.word(shot,label,tree=(label=='Wave'),minimum_y=300)
        subprocess.run(['wl-copy','--type','text/plain'],input=b'old text before PNG copy',env=session.env,check=True,timeout=5)
        session.click(x+10,y)
        wait_for(lambda: len(daemon.images)>index, 'PNG copy reaches the acknowledging daemon')
        def image_copied():
            result = subprocess.run(['wl-paste','--list-types'],env=session.env,capture_output=True,timeout=5)
            return 'image/png' in result.stdout.decode()
        wait_for(image_copied, 'audio PNG replaces stale text on the OS clipboard')
        png = subprocess.run(['wl-paste','--no-newline','--type','image/png'],env=session.env,capture_output=True,timeout=5,check=True).stdout
        assert (png,'image/png')==daemon.images[index]
        assert Image.open(io.BytesIO(png)).height==height
        (session.output/f'copied-analysis-{index}.png').write_bytes(png)
    session.observe('analysis-images-copied',['Selection info'])
    session.click(1110,72)
    session.observe('returned-from-audio',['tone.wav'])
    open_file(session,root,'music-three-track.mid','midi-row')
    session.observe('midi-studio',['music-three-track.mid','BPM'])
    subprocess.run(['wl-copy','--type','text/plain'],input=b'old text before MIDI copy',env=session.env,check=True,timeout=5)
    session.key('c','ctrl')
    wait_for(lambda: len(daemon.received)>2, 'MIDI export reaches the acknowledging daemon')
    def midi_copied():
        types = subprocess.run(['wl-paste','--list-types'],env=session.env,capture_output=True,timeout=5).stdout
        return b'text/uri-list' in types
    wait_for(midi_copied,'MIDI file replaces stale text on the OS clipboard')
    uri = private.run_checked(['wl-paste','--no-newline','--type','text/uri-list'],env=session.env).strip()
    path = Path(unquote(urlparse(uri).path))
    payload = path.read_bytes()
    assert str(path)==daemon.received[2] and payload.startswith(b'MThd') and payload.count(b'MTrk')>0
    (session.output/'copied-midi.json').write_text(json.dumps({'uri':uri,'bytes':len(payload)}))


def main():
    args = application_arguments(__doc__, Path(__file__).resolve().parent.parent/'target/release/clip-file-manager')
    output, root = media_fixture_directory(args)
    wav_fixture(root/'tone.wav',10,'stereo')
    midi_fixture(root)
    daemons = []
    def environment(env, children):
        start_private_audio(output,env,children)
        daemons.append(AudioDaemon(Path(env['XDG_RUNTIME_DIR']),output/'daemon.json'))
    try:
        return run_application_acceptance(args,output,root,lambda session,root:exercise(session,root,daemons[0]),'audio/MIDI files and waveform/FFT PNG clipboard after successful daemon acknowledgment',environment)
    finally:
        for daemon in daemons: daemon.close()


if __name__=='__main__':
    raise SystemExit(main())
