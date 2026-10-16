#!/usr/bin/env python3
"""Radio block metadata on private Weston/Niri; rfkill streams stay unopened."""
from pathlib import Path
from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance, private


def fixtures(root):
    radio=next(iter(sorted(Path('/sys/class/rfkill').iterdir())))
    (root/'a-radio').symlink_to(radio,target_is_directory=True)
    (root/'b-access').symlink_to('/dev/rfkill')
    (root/'c-soft').symlink_to(radio/'soft')
    (root/'notes.txt').write_text('Ordinary text beside read-only radio block metadata.\n')


def exercise(session,root):
    session.observe('01-radio-strips',['a-radio','b-access','Radio'])
    session.click(848,201)
    session.observe('02-mixed-folder',['a-radio','b-access','c-soft','notes.txt'])
    x,y=session.word('02-mixed-folder','a-radio')
    session.click(x+32,y+55)
    session.observe('03-radio-inspector',['Radio block state','Radio identity','Software block','Hardware block'])
    session.key('Escape')
    session.observe('04-browser-return',['a-radio','b-access'])
    x,y=session.word('04-browser-return','b-access')
    session.click(x+32,y+55)
    session.observe('05-access-inspector',['RFkill access stream','RFkill access policy'])
    for _ in range(2):
        private.run_checked([str(session.pointer),'scroll','1280','800','1050','600','700'],env=session.env)
    session.observe('05-access-topology',['Registered radio block controls','RFkill event/control interface'])
    session.key('Escape')
    session.observe('06-folder-return',['a-radio','b-access','c-soft'])
    x,y=session.word('06-folder-return','c-soft')
    session.click(x+16,y,delay=0.05)
    session.click(x+16,y)
    session.observe('07-soft-inspector',['Radio block state','Software block','Hardware block'])
    session.key('Escape')
    session.observe('08-final-folder',['a-radio','b-access','notes.txt'])


def main():
    args=application_arguments(__doc__,Path('target/release/clip-file-manager'))
    output,root=media_fixture_directory(args)
    fixtures(root)
    return run_application_acceptance(args,output,root,exercise,'Radio block metadata and unopened rfkill access')


if __name__=='__main__':
    raise SystemExit(main())
