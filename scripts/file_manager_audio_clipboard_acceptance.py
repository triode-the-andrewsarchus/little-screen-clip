#!/usr/bin/env python3
"""Audio Studio copies real file URIs even when the daemon only acknowledges handoff."""
import json
import subprocess
import wave
from pathlib import Path
from urllib.parse import unquote, urlparse
import ui_acceptance_capture as private
from ui_acceptance_fixtures import wav_fixture
from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance, wait_for
from file_manager_phase4_acceptance import open_file
from file_manager_media_audio import start_private_audio


class AudioDaemon(private.FakeTextPreviewDaemon):
    def __init__(self, runtime, log):
        self.received = []
        super().__init__(runtime, log)

    def _response(self, request):
        if isinstance(request, dict) and 'PushFile' in request:
            self.received.append(request['PushFile']['path'])
            # Deliberately leave the OS clipboard unchanged after acknowledging.
            return {'Success': {'message': 'Accepted audio'}}, 'audio_push'
        return super()._response(request)


def exercise(session, root, daemon):
    open_file(session, root, 'tone.wav', 'audio-row')
    session.observe('audio-studio', ['tone.wav', 'Selection info', 'Copy Track'])
    for index, button in enumerate([True, False]):
        if not button:
            private.run_checked([str(session.pointer), 'drag', '1280', '800', '350', '485', '650', '485'], env=session.env)
            session.observe('audio-selection', ['Copy Slice', 'Selection info'])
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
            assert 9.9<duration<10.1 if button else 2.5<duration<3.5, duration
        (session.output/f'copied-audio-{index}.json').write_text(json.dumps({'uri':uri,'seconds':duration,'channels':2}))


def main():
    args = application_arguments(__doc__, Path(__file__).resolve().parent.parent/'target/release/clip-file-manager')
    output, root = media_fixture_directory(args)
    wav_fixture(root/'tone.wav',10,'stereo')
    daemons = []
    def environment(env, children):
        start_private_audio(output,env,children)
        daemons.append(AudioDaemon(Path(env['XDG_RUNTIME_DIR']),output/'daemon.json'))
    try:
        return run_application_acceptance(args,output,root,lambda session,root:exercise(session,root,daemons[0]),'audio button/shortcut file clipboard with successful daemon acknowledgment',environment)
    finally:
        for daemon in daemons: daemon.close()


if __name__=='__main__':
    raise SystemExit(main())
