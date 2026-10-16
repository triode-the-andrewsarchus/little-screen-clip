#!/usr/bin/env python3
"""Powercap and regulator metadata on private Weston/Niri; no power changes."""
from pathlib import Path
from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance


def fixtures(root):
    zone=next(p for p in sorted(Path('/sys/class/powercap').iterdir()) if (p/'energy_uj').is_file())
    regulator=next(iter(sorted(Path('/sys/class/regulator').iterdir())))
    (root/'a-energy').symlink_to(zone,target_is_directory=True)
    (root/'b-regulator').symlink_to(regulator,target_is_directory=True)
    (root/'c-range').symlink_to(zone/'max_energy_range_uj')
    (root/'notes.txt').write_text('Ordinary text beside read-only power metadata.\n')


def exercise(session,root):
    session.observe('01-power-strips',['a-energy','b-regulator','Powercap','Regulator'])
    session.click(848,201)
    session.observe('02-mixed-folder',['a-energy','b-regulator','c-range','notes.txt'])
    x,y=session.word('02-mixed-folder','a-energy')
    session.click(x+32,y+55)
    session.observe('03-energy-inspector',['Measurements and limits','Energy domain','Energy counter','Energy counter range'])
    session.key('Escape')
    session.observe('04-browser-return',['a-energy','b-regulator'])
    x,y=session.word('04-browser-return','b-regulator')
    session.click(x+32,y+55)
    session.observe('05-regulator-inspector',['Regulator settings and constraints','Regulator name','Kernel enable-user count','Consumer-requested load'])
    session.key('Escape')
    session.observe('06-folder-return',['a-energy','b-regulator','c-range'])
    x,y=session.word('06-folder-return','c-range')
    session.click(x+16,y,delay=0.05)
    session.click(x+16,y)
    session.observe('07-range-inspector',['Measurements and limits','Energy counter range'])
    session.key('Escape')
    session.observe('08-final-folder',['a-energy','b-regulator','notes.txt'])


def main():
    args=application_arguments(__doc__,Path('target/release/clip-file-manager'))
    output,root=media_fixture_directory(args)
    fixtures(root)
    return run_application_acceptance(args,output,root,exercise,'Powercap counters and regulator settings')


if __name__=='__main__':
    raise SystemExit(main())
