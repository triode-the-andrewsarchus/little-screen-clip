#!/usr/bin/env python3
"""Inspect GPX maps and linked supplied-elevation profiles on private Weston/Niri."""
from pathlib import Path
import hashlib
import shutil
from PIL import Image
from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance, wait_for, private
from file_manager_phase4_acceptance import open_file, click_word


def map_pixels(session, shot):
    with Image.open(session.output/f"{shot}.png") as image:
        return image.crop((346,158,1182,574)).tobytes()


def exercise(session, root):
    original=hashlib.sha256((root/'harbor.gpx').read_bytes()).digest()
    session.observe('01-geographic-strip',['harbor.gpx','3 shapes','8 positions','Blue pier','Ferry route','Coastal track'])
    _,y=session.word('01-geographic-strip','harbor.gpx',minimum_y=240)
    session.click(860,y+45,delay=0.05)
    session.click(860,y+45,delay=0.05)
    session.observe('02-selected-route',['harbor.gpx','Harbor walk','Ferry route','2 positions','Spherical length','47.616000','-122.352000','Offline map'])
    click_word(session,'02-selected-route','Coastal',minimum_y=200)
    session.observe('03-track',['Coastal track','5 positions','2 segments','12.0','45.0','47.620000','-122.350000'])
    baseline=map_pixels(session,'03-track')
    private.run_checked([str(session.pointer),'drag','1280','800','1050','350','1120','390'],env=session.env)
    session.observe('03b-pan',['Coastal track','5 positions'])
    assert map_pixels(session,'03b-pan')!=baseline, 'Pan did not change the map'
    click_word(session,'03b-pan','Fit',minimum_y=200)
    session.observe('03c-fit',['Coastal track','5 positions'])
    assert map_pixels(session,'03c-fit')==baseline, 'Fit did not restore the original map after panning'
    private.run_checked([str(session.pointer),'drag','1280','800','187','233','250','233'],env=session.env)
    session.observe('03d-zoom',['Coastal track','5 positions'])
    assert map_pixels(session,'03d-zoom')!=baseline, 'Zoom did not change the map'
    click_word(session,'03d-zoom','Fit',minimum_y=200)
    session.observe('03e-fit',['Coastal track','5 positions'])
    assert map_pixels(session,'03e-fit')==baseline, 'Fit did not restore the original map after zooming'
    # Profile right edge in the fixed private viewport; hovering selects the last supplied sample.
    session.click(1160,700)
    session.observe('04-linked-position',['47.630000','-122.338000','Elevation 19.0','09:25:00'],preserve_pointer=True)
    click_word(session,'04-linked-position','Blue',minimum_y=200)
    session.observe('05-waypoint',['Blue pier','1 positions','47.620000','Meet at the blue sign'])
    click_word(session,'05-waypoint','Ferry',minimum_y=200)
    session.observe('06-route',['Ferry route','2 positions','0.0'])
    session.click(205,126)
    session.observe('07-rename',['Filename'])
    session.key('a','ctrl')
    private.run_checked(['wtype','renamed.gpx'],env=session.env)
    session.key('Return')
    wait_for(lambda:(root/'renamed.gpx').exists(),'geographic rename')
    assert hashlib.sha256((root/'renamed.gpx').read_bytes()).digest()==original
    session.observe('08-renamed',['renamed.gpx','Ferry route'])
    session.click(1110,70)
    session.observe('09-folder',['renamed.gpx','3 shapes','notes.txt'])
    open_file(session,root,'renamed.gpx','10-reopen')
    session.observe('11-reopened',['Harbor walk','Coastal track','5 positions','2 segments'])
    session.click(1110,70)
    session.observe('12-returned',['renamed.gpx','notes.txt'])


def main():
    repo=Path(__file__).resolve().parents[1]
    args=application_arguments(__doc__,repo/'target/release/clip-file-manager')
    output,root=media_fixture_directory(args)
    shutil.copyfile(repo/'crates/little-clip-core/tests/fixtures/geography/harbor.gpx',root/'harbor.gpx')
    (root/'notes.txt').write_text('An ordinary neighboring file.\n')
    return run_application_acceptance(args,output,root,exercise,'GPX feature strip selection, offline map, separate track segments, linked elevation cursor, waypoints, guarded rename and reopen')


if __name__=='__main__':
    raise SystemExit(main())
