#!/usr/bin/env python3
"""PTP/PPS metadata on private Weston/Niri; clocks and event FIFOs stay unopened."""
from pathlib import Path
from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance


def fixtures(root):
    clock=next(iter(sorted(Path('/sys/class/ptp').iterdir())))
    source=next(iter(sorted(Path('/sys/class/pps').iterdir())))
    (root/'a-clock').symlink_to(clock,target_is_directory=True)
    (root/'b-pulses').symlink_to(source,target_is_directory=True)
    (root/'c-access').symlink_to(Path('/dev')/clock.name)
    (root/'notes.txt').write_text('Ordinary text beside read-only clock capabilities.\n')


def exercise(session,root):
    session.observe('01-clock-strips',['a-clock','b-pulses','PTP','PPS'])
    session.click(848,201)
    session.observe('02-mixed-folder',['a-clock','b-pulses','c-access','notes.txt'])
    x,y=session.word('02-mixed-folder','a-clock')
    session.click(x+32,y+55)
    session.observe('03-ptp-inspector',['PTP clock capabilities','PTP clock friendly name','Maximum frequency adjustment'])
    session.key('Escape')
    session.observe('04-browser-return',['a-clock','b-pulses','c-access'])
    x,y=session.word('04-browser-return','b-pulses')
    session.click(x+32,y+55)
    session.observe('05-pps-inspector',['Stored PPS observations','PPS source name','mode bits (hex)','Stored assert timestamp','Stored clear timestamp'])
    session.key('Escape')
    session.observe('06-folder-return',['a-clock','b-pulses','c-access'])
    x,y=session.word('06-folder-return','c-access')
    session.click(x+32,y+55)
    session.observe('07-access-inspector',['PTP clock capabilities','PTP clock friendly name','Maximum frequency adjustment'])
    session.key('Escape')
    session.observe('08-final-folder',['a-clock','b-pulses','notes.txt'])


def main():
    args=application_arguments(__doc__,Path('target/release/clip-file-manager'))
    output,root=media_fixture_directory(args)
    fixtures(root)
    return run_application_acceptance(args,output,root,exercise,'PTP capabilities and stored PPS observations')


if __name__=='__main__':
    raise SystemExit(main())
