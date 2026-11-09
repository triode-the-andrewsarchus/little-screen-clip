#!/usr/bin/env python3
"""Inspect saved MIME messages on private Weston/Niri without contacting mail services."""
from pathlib import Path
import hashlib
import shutil
from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance, wait_for, private
from file_manager_phase4_acceptance import open_file, click_word


def exercise(session, root):
    original=hashlib.sha256((root/'harbor.eml').read_bytes()).digest()
    session.observe('01-message-strip',['harbor.eml','Harbor meeting','2 attachments','route.txt','tide-data.bin'])
    _,y=session.word('01-message-strip','harbor.eml',minimum_y=240)
    session.click(470,y+75,delay=0.05)
    session.click(470,y+75,delay=0.05)
    session.observe('02-selected-attachment',['harbor.eml','Harbor meeting','blue pier','Attachments (2)','application/octet-stream','16 decoded bytes','Source'])
    click_word(session,'02-selected-attachment','Source',minimum_y=150)
    session.observe('03-source',['MIME-Version','multipart/mixed','From'])
    click_word(session,'03-source','Message',minimum_y=150)
    session.observe('04-message',['Harbor plan','route.txt','application/octet-stream','16 decoded bytes'])
    click_word(session,'04-message','route.txt',minimum_y=200)
    session.observe('05-attachment-details',['text/plain','19 decoded bytes','Harbor plan'])
    # Filename center verified in the private header; sparse OCR can misread its extension.
    session.click(205,126)
    session.observe('06-rename',['Filename'])
    session.key('a','ctrl')
    private.run_checked(['wtype','renamed.eml'],env=session.env)
    session.key('Return')
    wait_for(lambda:(root/'renamed.eml').exists(),'message rename')
    assert hashlib.sha256((root/'renamed.eml').read_bytes()).digest()==original
    session.observe('07-renamed',['renamed.eml','Harbor plan','text/plain'])
    session.click(1110,70)
    session.observe('08-folder',['renamed.eml','Harbor meeting','2 attachments','notes.txt'])
    open_file(session,root,'renamed.eml','09-reopen')
    session.observe('10-reopened',['Harbor plan','Attachments (2)','text/plain'])
    session.click(1110,70)
    session.observe('11-returned',['renamed.eml','notes.txt'])


def main():
    repo=Path(__file__).resolve().parents[1]
    args=application_arguments(__doc__,repo/'target/release/clip-file-manager')
    output,root=media_fixture_directory(args)
    shutil.copyfile(repo/'crates/little-clip-core/tests/fixtures/email/harbor.eml',root/'harbor.eml')
    (root/'notes.txt').write_text('An ordinary neighboring file.\n')
    return run_application_acceptance(args,output,root,exercise,'message strip attachment selection, decoded body, original source, attachment metadata, rename and reopen')


if __name__=='__main__':
    raise SystemExit(main())
