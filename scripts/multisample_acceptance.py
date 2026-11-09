#!/usr/bin/env python3
"""Inspect sample maps and audition owned audio on private Weston/Niri/PipeWire."""
from pathlib import Path
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


def exercise(session,root):
    original=hashlib.sha256((root/'harbor.sfz').read_bytes()).digest()
    session.observe('01-strips',['0-preview.dslibrary','3 regions'])
    open_file(session,root,'harbor.sfz','02-sfz-open')
    session.observe('03-sfz-map',['SFZ instrument','3 regions','Soft keys','Keys 48','root 60','Velocity 0','Loop 10','Key / velocity map','Audition raw sample'])
    # Bright velocity layer in the fixed private map viewport.
    session.click(735,300)
    session.observe('04-selected-layer',['Bright keys','Velocity 64','127','bright.wav'])
    click_word(session,'04-selected-layer','Audition',minimum_y=500)
    wait_for(lambda:heard(session),'SFZ raw-sample audio')
    session.observe('05-sfz-audio',['Bright keys','2.00 s','no pitch/DSP'])
    session.click(205,126)
    session.observe('06-rename',['Filename'])
    session.key('a','ctrl')
    private.run_checked(['wtype','renamed.sfz'],env=session.env)
    session.key('Return')
    wait_for(lambda:(root/'renamed.sfz').exists(),'SFZ rename')
    assert hashlib.sha256((root/'renamed.sfz').read_bytes()).digest()==original
    session.observe('07-renamed',['renamed.sfz','Bright keys'])
    session.click(1110,70)
    wait_for(lambda:quiet(session),'stopped sample audition')
    session.observe('08-folder',['Ctrl+L','Folders','List'])
    open_file(session,root,'harbor.sf2','09-sf2-open')
    session.observe('10-sf2-map',['Harbor Keys','2 regions','Soft tone','Keys 48','root 60','Loop 10'])
    session.click(735,300)
    session.observe('11-sf2-layer',['Bright tone','Velocity 64','127','22050 Hz'])
    wait_for(lambda:quiet(session),'quiet output before embedded audition')
    click_word(session,'11-sf2-layer','Audition',minimum_y=500)
    wait_for(lambda:heard(session),'embedded SoundFont PCM audio')
    session.observe('12-sf2-audio',['Bright tone','2.00 s'])
    # Preset selection is a parameter combo in the shared left panel.
    session.click(205,207)
    session.observe('13-presets',['Harbor Bass'])
    click_word(session,'13-presets','Bass',minimum_y=200)
    session.observe('14-bass',['Harbor Bass','1 regions','Keys 36','root 48'])
    session.click(1110,70)
    session.observe('15-folder',['Ctrl+L','Folders','List'])
    open_file(session,root,'legacy.nki','16-legacy-open')
    session.observe('17-legacy',['Harbor legacy','2 regions','soft.wav','Keys 48','root 60'])
    wait_for(lambda:quiet(session),'quiet output before NKI audition')
    click_word(session,'17-legacy','Audition',minimum_y=500)
    wait_for(lambda:heard(session),'legacy NKI external sample audio')
    session.observe('18-legacy-audio',['Harbor legacy','2.00 s'])
    session.click(1110,70)
    open_file(session,root,'reference-modern.nki','19-modern-open')
    session.observe('20-modern',['FM8 Jam','61 regions','root 71'])
    session.click(1110,70)
    open_file(session,root,'renamed.sfz','21-reopen')
    session.observe('22-reopened',['renamed.sfz','3 regions','Soft keys'])
    session.click(1110,70)
    session.observe('23-returned',['Ctrl+L','Folders','List'])
    for index,(filename,title,count,sample) in enumerate([
        ('harbor.dspreset','Harbor Decent',3,'bright.wav'),
        ('harbor.multisample','Harbor packed',3,'bright.wav'),
        ('harbor.dslibrary','Harbor library',3,'bright.wav'),
        ('mapping.xml','Harbor Decent',3,'bright.wav'),
        ('harbor.exs','Harbor EXS',2,'bright.wav'),
        ('harbor.xrni','Harbor Renoise',3,'Sample01'),
        ('native-compressed.nki','Harbor compressed',2,'bright.ncw'),
        ('compressed.sfz','SFZ instrument',2,'bright.ncw'),
    ],24):
        tag=f'{index:02d}-native-mapping'
        open_file(session,root,filename,tag+'-open')
        session.observe(tag,[title,f'{count} regions','Keys 48','Velocity 0','Key / velocity map'])
        session.click(735,300)
        session.observe(tag+'-layer',[title,'Velocity 64','127',sample])
        wait_for(lambda:quiet(session),'quiet output before packed sample audition')
        click_word(session,tag+'-layer','Audition',minimum_y=500)
        wait_for(lambda:heard(session),'native mapping sample audio')
        session.observe(tag+'-audio',[title,'2.00 s'])
        session.click(1110,70)
        wait_for(lambda:quiet(session),'stopped native sample audition')
    open_file(session,root,'ordinary.xml','28-generic-xml-open')
    session.observe('29-generic-xml',['ordinary.xml','Ordinary XML remains text'])
    session.click(1110,70)
    open_file(session,root,'ordinary.exs','30-elixir-open')
    session.observe('31-elixir',['ordinary.exs','Ordinary Elixir script'])
    session.click(1110,70)


def main():
    repo=Path(__file__).resolve().parents[1]
    args=application_arguments(__doc__,repo/'target/release/clip-file-manager')
    output,root=media_fixture_directory(args)
    for source in (repo/'crates/little-clip-core/tests/fixtures/multisample').iterdir():
        shutil.copyfile(source,root/source.name)
    shutil.copyfile(repo/'crates/little-clip-nki/tests/fixtures/kontakt680.nki',root/'reference-modern.nki')
    shutil.copyfile(root/'harbor.dslibrary',root/'0-preview.dslibrary')
    shutil.copyfile(root/'harbor.dspreset',root/'mapping.xml')
    (root/'ordinary.exs').write_text('IO.puts("Ordinary Elixir script")\n')
    (root/'ordinary.xml').write_text('<notes>Ordinary XML remains text</notes>\n')
    (root/'notes.txt').write_text('An ordinary neighboring file.\n')
    def prepare(env,children):
        start_private_audio(output,env,children)
    return run_application_acceptance(args,output,root,exercise,'Native sampler mapping strips, source and velocity selection, presets, private WAV/NCW/archive audio, isolated NKI/EXS helper, guarded rename, reopen and ambiguous-name fallbacks',prepare_environment=prepare)


if __name__=='__main__':
    raise SystemExit(main())
