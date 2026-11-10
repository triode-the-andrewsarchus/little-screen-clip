#!/usr/bin/env python3
"""Inspect sample maps and audition owned audio on private Weston/Niri/PipeWire."""
from pathlib import Path
import io
import wave
import zipfile
import traceback
import subprocess
import hashlib
import shutil
from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance, wait_for, private
from file_manager_phase4_acceptance import click_word
from file_manager_phase5_acceptance import audio_tail
from file_manager_media_audio import start_private_audio

def open_file(session, root, filename, shot):
    # Exercise literal-name browser navigation so virtual rows and OCR spelling
    # do not determine which owned fixture is opened.
    session.observe(shot+'-browser',['Ctrl+L','Folders'])
    session.key('l','ctrl')
    session.key('Escape')
    private.run_checked(['wtype',filename],env=session.env)
    if filename in ['mapping.xml','harbor.exs']:
        count=3 if filename=='mapping.xml' else 2
        session.observe(shot+'-row',[f'{count} regions'])
    session.key('Return')
    session.observe(shot,[filename])


def heard(session):
    return any(abs(value)>2000 for value in audio_tail(session)[0])


def quiet(session):
    samples,size=audio_tail(session)
    return size>32000 and samples and max(abs(value) for value in samples)<200


class MidiFixture:
    def __init__(self, session):
        self.process=subprocess.Popen([str(session.output/"midi-fixture"),str(session.process.pid)],stdin=subprocess.PIPE,stdout=subprocess.PIPE,text=True)
    def send(self,status,key=0,value=0):
        self.process.stdin.write(f"{status} {key} {value}\n")
        self.process.stdin.flush()
        return int(self.process.stdout.readline())
    def close(self):
        self.process.stdin.close()
        self.process.wait(timeout=5)


def exercise(session, root):
    midi=MidiFixture(session)
    try:
        exercise_instrument(session,root,midi)
    except Exception:
        traceback.print_exc()
        raise
    finally:
        midi.close()


def exercise_instrument(session,root,midi):
    original = hashlib.sha256((root/'harbor.sfz').read_bytes()).digest()
    session.observe('01-coverage', ['0-preview.dslibrary', '3 regions'])
    session.click(962, 290)
    wait_for(lambda: heard(session), 'inline high-velocity audio')
    session.observe('02-inline-audio', ['Folders', '0-preview.dslibrary'])
    session.click(426, 292)
    wait_for(lambda: quiet(session), 'inline Stop control')
    # Header click focuses MIDI without playing a pointer note.
    session.click(420,262)
    wait_for(lambda:midi.send(0)==1,'owned MIDI source connection')
    assert midi.send(0x90,60,127)==1
    assert midi.send(0x90,64,127)==1
    wait_for(lambda:heard(session),'polyphonic inline MIDI')
    assert midi.send(0x80,60,0)==1
    assert midi.send(0x90,64,0)==1
    wait_for(lambda:quiet(session),'MIDI note-off and zero velocity')
    midi.send(0xb0,64,127)
    midi.send(0x90,60,127)
    wait_for(lambda:heard(session),'sustain note')
    midi.send(0x80,60,0)
    released_at=audio_tail(session)[1]
    wait_for(lambda:audio_tail(session)[1]>released_at+8000,'audio capture after sustained release')
    sustained_peak=max(map(abs,audio_tail(session)[0][-1600:]),default=0)
    assert sustained_peak>1000, f"Sustained note went silent: peak={sustained_peak}"
    midi.send(0xb0,64,0)
    wait_for(lambda:quiet(session),'sustain pedal release')
    midi.send(0x90,60,127)
    wait_for(lambda:heard(session),'held note before focus change')
    session.click(150,210)
    wait_for(lambda:quiet(session),'outside click releases MIDI')
    wait_for(lambda:midi.send(0)==0,'outside click disconnects MIDI route')

    open_file(session,root,'harbor.sfz','03-sfz-open')
    session.observe('04-mapping',['At played pitch','velocity','Mapping'])
    wait_for(lambda:midi.send(0)==1,'inspector MIDI connection')
    midi.send(0x90,60,127)
    wait_for(lambda:heard(session),'inspector MIDI note')
    midi.send(0xb0,123,0)
    wait_for(lambda:quiet(session),'all-notes-off')
    session.click(915,540)
    wait_for(lambda:max(map(abs,audio_tail(session)[0]),default=0)>100,'soft map recording')
    soft_peak=max(map(abs,audio_tail(session)[0]))
    session.observe('05-soft',['soft.wav','Velocity 0','63'])
    session.click(500,685)
    wait_for(lambda:quiet(session),'stop before same-layer dynamics check')
    session.click(915,470)
    wait_for(lambda:max(map(abs,audio_tail(session)[0]),default=0)>soft_peak*2.5,'same recording responds to velocity')
    session.observe('05-medium',['soft.wav','Velocity 0','63'])
    session.click(915,300)
    wait_for(lambda:heard(session),'hard map recording')
    session.observe('06-hard',['bright.wav','Velocity 64','127'])
    session.click(390,174)
    session.observe('07-harmony',['Major','Half-dim','Inversion'])
    session.click(764,484)
    wait_for(lambda:heard(session),'major chord')
    session.observe('08-major',['C4','E4','G4'])
    session.key('2')
    session.observe('09-minor',['C4','G4'])
    session.key('7')
    session.observe('10-seventh',['Seventh'])
    session.key('9')
    session.observe('11-lower-root',['C3'])
    session.key('0')
    session.observe('12-unmapped-octave',['No recording for C5'])
    session.key('0')
    session.key('9')
    session.key('7')
    session.key('1')
    # Wheel changes scale, not the notes of the sounding chord.
    private.run_checked([str(session.pointer),'scroll','1280','800','764','484','120'],env=session.env)
    session.observe('13-zoom',['C4','E4','G4'])
    session.click(460,174)
    session.observe('14-pads',['original pitch','Soft keys','Bright keys'])
    session.click(438,300)
    wait_for(lambda:heard(session),'named pad audition')

    session.click(205, 126)
    session.observe('13-rename', ['Filename'])
    session.key('a', 'ctrl')
    private.run_checked(['wtype', 'renamed.sfz'], env=session.env)
    session.key('Return')
    wait_for(lambda: (root/'renamed.sfz').exists(), 'sampler rename')
    assert hashlib.sha256((root/'renamed.sfz').read_bytes()).digest() == original
    session.click(1110, 70)
    wait_for(lambda: quiet(session), 'return to folder stops audio')

    # Every supported source path still auditions through the shared keyboard.
    for index, (filename, title) in enumerate([
        ('harbor.sf2', 'Harbor Keys'),
        ('legacy.nki', 'Harbor legacy'),
        ('harbor.dspreset', 'Harbor Decent'),
        ('harbor.multisample', 'Harbor packed'),
        ('harbor.dslibrary', 'Harbor library'),
        ('mapping.xml', 'Harbor Decent'),
        ('harbor.exs', 'Harbor EXS'),
        ('harbor.xrni', 'Harbor Renoise'),
        ('native-compressed.nki', 'Harbor compressed'),
        ('compressed.sfz', 'SFZ instrument'),
    ], 14):
        tag = f'{index:02d}-source'
        open_file(session, root, filename, tag+'-open')
        session.observe(tag, [title, 'Harmony', 'Mapping'])
        # Some instruments span C3-B4, others C2-B4. Choosing the raw source
        # Replay button avoids relying on that file's keyboard width.
        click_word(session, tag, 'Replay', minimum_y=600)
        wait_for(lambda: heard(session), filename+' source audio')
        session.observe(tag+'-audio', [title, '2.00 s'])
        session.click(1110, 70)
        wait_for(lambda: quiet(session), filename+' stopped on navigation')
    open_file(session, root, 'reference-modern.nki', '24-modern')
    session.observe('25-modern-mapping', ['FM8 Jam', '61 regions'])
    session.click(1110, 70)
    for filename, text in [('ordinary.xml', 'Ordinary XML remains text'), ('ordinary.exs', 'Ordinary Elixir script')]:
        open_file(session, root, filename, filename+'-open')
        session.observe(filename+'-fallback', [text])
        session.click(1110, 70)


def main():
    repo=Path(__file__).resolve().parents[1]
    args=application_arguments(__doc__,repo/'target/release/clip-file-manager')
    output,root=media_fixture_directory(args)
    subprocess.run(["cc","-Wall","-Wextra","-Werror",str(repo/"scripts/multisample_midi_fixture.c"),"-lasound","-o",str(output/"midi-fixture")],check=True)
    for source in (repo/'crates/little-clip-core/tests/fixtures/multisample').iterdir():
        shutil.copyfile(source,root/source.name)
    shutil.copyfile(repo/'crates/little-clip-nki/tests/fixtures/kontakt680.nki',root/'reference-modern.nki')
    # Long owned tones keep MIDI release/sustain checks independent of capture buffering.
    with zipfile.ZipFile(root/'harbor.dslibrary') as original, zipfile.ZipFile(root/'0-preview.dslibrary','w') as extended:
        for name in original.namelist():
            data=original.read(name)
            if name.endswith('.wav'):
                with wave.open(io.BytesIO(data),'rb') as source:
                    params=source.getparams()
                    frames=source.readframes(source.getnframes())
                output_audio=io.BytesIO()
                with wave.open(output_audio,'wb') as target:
                    target.setparams(params)
                    target.writeframes(frames*4)
                data=output_audio.getvalue()
            elif name.endswith('.dspreset'):
                data=data.replace(b'end="44099"',b'end="176399"')
            extended.writestr(name,data)
    shutil.copyfile(root/'harbor.dspreset',root/'mapping.xml')
    (root/'ordinary.exs').write_text('IO.puts("Ordinary Elixir script")\n')
    (root/'ordinary.xml').write_text('<notes>Ordinary XML remains text</notes>\n')
    (root/'notes.txt').write_text('An ordinary neighboring file.\n')
    def prepare(env,children):
        env["LITTLE_CLIP_MIDI_INPUT_PREFIX"]="Private multisample acceptance source:Owned fixture "
        start_private_audio(output,env,children)
    return run_application_acceptance(args,output,root,exercise,'Inline coverage and audio, harmonic chords and missing voicings, zoom, velocity layers, mapping/pads and focused MIDI with sustain, private WAV/NCW/archive audio, guarded rename and ambiguous-name fallbacks',prepare_environment=prepare)


if __name__=='__main__':
    raise SystemExit(main())
