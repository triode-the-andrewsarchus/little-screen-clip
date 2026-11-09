#!/usr/bin/env python3
"""Exercise Windows Media waveforms, playback and studio rename on private Weston/Niri."""
from pathlib import Path
import hashlib
import shutil
from PIL import Image, ImageOps
from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance, wait_for, private
from file_manager_phase4_acceptance import open_file
from file_manager_phase5_acceptance import audio_tail
from file_manager_media_audio import start_private_audio


def include_clock_ocr(session):
    original=session.capture_text
    def capture(name,path,expected):
        text=original(name,path,expected)
        if 'Selection info' in expected:
            output=session.output/f'{name}-clock.png'
            with Image.open(path) as image:
                crop=ImageOps.grayscale(image.crop((530,110,752,155)))
                if crop.getpixel((0,0))<128:
                    crop=ImageOps.invert(crop)
                ImageOps.autocontrast(crop).resize((1110,225)).save(output)
            text+='\n'+private.run_checked(['tesseract',str(output),'stdout','--psm','7'],env=session.env)
        return text
    session.capture_text=capture


def exercise(session, root):
    include_clock_ocr(session)
    original=hashlib.sha256((root/'tones.wma').read_bytes()).digest()
    session.observe('01-folder',['tones.wma','mono.wma'])
    session.click(120,160)
    open_file(session,root,'tones.wma','02-row')
    text=session.observe('03-studio',['tones.wma','Selection info'],expected_any=['00:00:03.','00:00:04.'])
    assert 'audio analysis unavailable' not in text.lower()
    session.key('space')
    wait_for(lambda:any(abs(value)>2000 for value in audio_tail(session)[0]),'Windows Media stereo playback')
    session.observe('04-played',['tones.wma','Selection info'])
    session.key('space')
    session.click(205,126)
    session.observe('05-rename',['Filename'])
    session.key('a','ctrl')
    private.run_checked(['wtype','renamed.wma'],env=session.env)
    session.key('Return')
    wait_for(lambda:(root/'renamed.wma').exists(),'audio rename')
    session.observe('06-renamed',['renamed.wma','Selection info'])
    assert hashlib.sha256((root/'renamed.wma').read_bytes()).digest()==original
    session.click(1110,70)
    session.observe('07-folder',['renamed.wma','mono.wma'])
    open_file(session,root,'mono.wma','08-mono-row')
    text=session.observe('09-mono',['mono.wma','Selection info'],expected_any=['00:00:01.','00:00:02.'])
    assert 'audio analysis unavailable' not in text.lower()
    session.click(1110,70)
    session.observe('10-returned',['renamed.wma','mono.wma'])
    open_file(session,root,'renamed.wma','11-reopen')
    session.observe('12-reopened',['renamed.wma','Selection info'],expected_any=['00:00:03.','00:00:04.'])
    session.click(1110,70)
    session.observe('13-returned',['renamed.wma','mono.wma'])


def main():
    repo=Path(__file__).resolve().parents[1]
    args=application_arguments(__doc__,repo/'target/release/clip-file-manager')
    output,root=media_fixture_directory(args)
    for name in ['tones.wma','mono.wma']:
        shutil.copyfile(repo/'crates/little-clip-core/tests/fixtures/audio'/name,root/name)
    return run_application_acceptance(args,output,root,exercise,'Windows Media stereo/mono waveform analysis, private audio playback, guarded rename and reopen',lambda env,children:start_private_audio(output,env,children))


if __name__=='__main__':
    raise SystemExit(main())
