#!/usr/bin/env python3
"""Inspect SQLite table data and schema on private Weston/Niri, preserving the source."""
from pathlib import Path
import hashlib
import shutil
from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance, wait_for, private
from file_manager_phase4_acceptance import open_file, click_word


def exercise(session, root):
    original=hashlib.sha256((root/'harbor.sqlite').read_bytes()).digest()
    session.observe('01-schema-strip',['harbor.sqlite','3 objects','ports','sailings','summary'])
    _,y=session.word('01-schema-strip','harbor.sqlite',minimum_y=240)
    session.click(620,y+45,delay=0.05)
    session.click(620,y+45,delay=0.05)
    session.observe('02-selected-table',['harbor.sqlite','Rows','Schema','sailings','Morning crossing','200','more available','User version 7'])
    click_word(session,'02-selected-table','Next',minimum_y=200)
    session.observe('03-next-page',['201','205','Evening arrival'])
    click_word(session,'03-next-page','Previous',minimum_y=200)
    session.observe('04-previous-page',['Morning crossing','200','more available'])
    click_word(session,'04-previous-page','Schema',minimum_y=150)
    session.observe('05-schema',['sailings','Declared type','Primary key','Declared foreign keys','port_id','ports','Stored definition'])
    # Referenced-table button center verified in the private schema screenshot.
    session.click(450,572)
    session.observe('05b-related-table',['ports','Not null','CREATE TABLE ports'])
    # The owned fixture's second object row, verified in the schema screenshot.
    session.click(220,342)
    session.observe('05c-returned-table',['sailings','Declared foreign keys','port_id'])
    click_word(session,'05c-returned-table','Rows',minimum_y=150)
    session.observe('06-rows',['Morning crossing','manifest'])
    click_word(session,'06-rows','ports',minimum_y=250)
    session.observe('07-other-table',['North harbor','Café pier','2'])
    click_word(session,'07-other-table','summary',minimum_y=250)
    session.observe('08-view',['metadata-only','views','not evaluated'])
    click_word(session,'08-view','Schema',minimum_y=150)
    session.observe('09-view-schema',['CREATE VIEW','summary','SELECT name FROM ports'])
    session.click(205,126)
    session.observe('10-rename',['Filename'])
    session.key('a','ctrl')
    private.run_checked(['wtype','renamed.sqlite'],env=session.env)
    session.key('Return')
    wait_for(lambda:(root/'renamed.sqlite').exists(),'database rename')
    assert hashlib.sha256((root/'renamed.sqlite').read_bytes()).digest()==original
    session.observe('11-renamed',['renamed.sqlite','CREATE VIEW'])
    session.click(1110,70)
    session.observe('12-folder',['renamed.sqlite','3 objects','notes.txt'])
    open_file(session,root,'renamed.sqlite','13-reopen')
    session.observe('14-reopened',['North harbor','User version 7','read only'])
    session.click(1110,70)
    session.observe('15-returned',['renamed.sqlite','notes.txt'])
    assert sorted(path.name for path in root.iterdir())==['notes.txt','renamed.sqlite']


def main():
    repo=Path(__file__).resolve().parents[1]
    args=application_arguments(__doc__,repo/'target/release/clip-file-manager')
    output,root=media_fixture_directory(args)
    shutil.copyfile(repo/'crates/clip-file-manager/tests/fixtures/database/harbor.sqlite',root/'harbor.sqlite')
    (root/'notes.txt').write_text('An ordinary neighboring file.\n')
    return run_application_acceptance(args,output,root,exercise,'schema strip selection, bounded row paging, table switching, declared schema, metadata-only views, guarded rename and reopen without sidecar writes')


if __name__=='__main__':
    raise SystemExit(main())
