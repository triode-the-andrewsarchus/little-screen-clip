#!/usr/bin/env python3
"""Inspect GeoJSON shapes and attributes on private Weston/Niri."""
from pathlib import Path
import hashlib
import shutil
from PIL import Image
from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance, wait_for, private
from file_manager_phase4_acceptance import open_file, click_word


def pixels(session, shot):
    with Image.open(session.output/f'{shot}.png') as image:
        return image.crop((346,158,1182,700)).tobytes()


def exercise(session, root):
    original=hashlib.sha256((root/'harbor.geojson').read_bytes()).digest()
    session.observe('01-strip',['harbor.geojson','4 shapes','15 positions','Harbor park','Survey route'])
    open_file(session,root,'harbor.geojson','02-open')
    session.observe('03-polygon',['Harbor survey','Harbor park','10 positions','2 segments','Offline map','Public footpath'])
    baseline=pixels(session,'03-polygon')
    private.run_checked([str(session.pointer),'drag','1280','800','1050','350','1120','390'],env=session.env)
    session.observe('04-pan',['Harbor park','10 positions'])
    assert pixels(session,'04-pan')!=baseline
    click_word(session,'04-pan','Fit',minimum_y=200)
    session.observe('05-fit',['Harbor park'])
    assert pixels(session,'05-fit')==baseline
    click_word(session,'05-fit','Survey',minimum_y=250)
    session.observe('06-line',['Survey route','3 positions','Supplied elevation','12.0','28.0'])
    click_word(session,'06-line','Blue',minimum_y=250)
    session.observe('07-points',['Blue pier','2 positions','Meet at the blue sign'])
    click_word(session,'07-points','Unplaced',minimum_y=250)
    session.observe('08-null',['Unplaced record','0 positions','Awaiting coordinates','Null'])
    session.click(205,126)
    session.observe('09-rename',['Filename'])
    session.key('a','ctrl')
    private.run_checked(['wtype','renamed.geojson'],env=session.env)
    session.key('Return')
    wait_for(lambda:(root/'renamed.geojson').exists(),'GeoJSON rename')
    assert hashlib.sha256((root/'renamed.geojson').read_bytes()).digest()==original
    session.observe('10-renamed',['renamed.geojson','Unplaced record'])
    session.click(1110,70)
    session.observe('11-folder',['renamed.geojson','4 shapes','notes.txt'])
    open_file(session,root,'renamed.geojson','12-reopen')
    session.observe('13-reopened',['Harbor park','10 positions'])
    session.click(1110,70)
    session.observe('14-returned',['renamed.geojson','notes.txt'])


def main():
    repo=Path(__file__).resolve().parents[1]
    args=application_arguments(__doc__,repo/'target/release/clip-file-manager')
    output,root=media_fixture_directory(args)
    shutil.copyfile(repo/'crates/little-clip-core/tests/fixtures/geography/harbor.geojson',root/'harbor.geojson')
    (root/'notes.txt').write_text('An ordinary neighboring file.\n')
    return run_application_acceptance(args,output,root,exercise,'GeoJSON polygon rings, map pan/fit, lines/elevation, multipoints, null geometry properties, guarded rename and reopen')


if __name__=='__main__':
    raise SystemExit(main())
