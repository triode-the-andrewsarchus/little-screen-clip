#!/usr/bin/env python3
"""Inspect a calendar in Files on private Weston/Niri, without executing declarations."""
from pathlib import Path
import hashlib
import shutil
from PIL import Image
from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance, wait_for, private
from file_manager_phase4_acceptance import open_file, click_word


def exercise(session, root):
    original=hashlib.sha256((root/'harbor.ics').read_bytes()).digest()
    session.observe('01-calendar-strip',['harbor.ics','5 events','UTC','2 shown'])
    _,y=session.word('01-calendar-strip','harbor.ics',minimum_y=240)
    session.click(800,y+65,delay=0.05)
    session.click(800,y+65,delay=0.05)
    session.observe('02-selected-event',['harbor.ics','Agenda','Timeline','Source','Tide watch','Cancelled','12:00:00'])
    click_word(session,'02-selected-event','Timeline',minimum_y=150)
    session.observe('03-timeline',['UTC','recurring instances are not expanded','10:30:00','Tide watch'])
    # Both events occupy this time coordinate, on separate overlap lanes.
    with Image.open(session.output/'03-timeline.png') as image:
        first=image.convert('RGB').getpixel((500,360))
        second=image.convert('RGB').getpixel((500,392))
        assert first[2]>first[0]+20 and second[0]>second[2]+20, (first,second)
    click_word(session,'03-timeline','Agenda',minimum_y=150)
    session.observe('04-agenda',['Harbor holiday','Floating meeting','Local workshop'])
    click_word(session,'04-agenda','holiday',minimum_y=200)
    session.observe('05-all-day',['Harbor holiday','Ends (exclusive)','2026-11-12','Derived from duration'])
    click_word(session,'05-all-day','workshop',minimum_y=200)
    session.observe('06-local',['Local workshop','Harbor/Local','Recurrence','Event properties'])
    click_word(session,'06-local','Timeline',minimum_y=150)
    session.observe('06b-named-timeline',['Harbor/Local','Local workshop','2026-11-13','09:00:00','10:00:00'])
    click_word(session,'06b-named-timeline','Source',minimum_y=150)
    session.observe('07-source',['BEGIN:VCALENDAR','VERSION:2.0','PRODID'])
    click_word(session,'07-source','Agenda',minimum_y=150)
    session.click(205,126)
    session.observe('08-rename',['Filename'])
    session.key('a','ctrl')
    private.run_checked(['wtype','renamed.ics'],env=session.env)
    session.key('Return')
    wait_for(lambda:(root/'renamed.ics').exists(),'calendar rename')
    assert hashlib.sha256((root/'renamed.ics').read_bytes()).digest()==original
    session.observe('09-renamed',['renamed.ics','Local workshop'])
    session.click(1110,70)
    session.observe('10-folder',['renamed.ics','5 events','notes.txt'])
    open_file(session,root,'renamed.ics','11-reopen')
    session.observe('12-reopened',['Agenda','Harbor crossing','Tide watch','5 events'])
    session.click(1110,70)
    session.observe('13-returned',['renamed.ics','notes.txt'])


def main():
    repo=Path(__file__).resolve().parents[1]
    args=application_arguments(__doc__,repo/'target/release/clip-file-manager')
    output,root=media_fixture_directory(args)
    shutil.copyfile(repo/'crates/little-clip-core/tests/fixtures/calendar/harbor.ics',root/'harbor.ics')
    (root/'notes.txt').write_text('An ordinary neighboring file.\n')
    return run_application_acceptance(args,output,root,exercise,'event strip selection, agenda, overlap timeline, separate time bases, exclusive all-day end, source, guarded rename and reopen')


if __name__=='__main__':
    raise SystemExit(main())
