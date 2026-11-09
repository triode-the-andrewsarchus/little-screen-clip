#!/usr/bin/env python3
"""Inspect and scrub toolpaths on private Weston/Niri, without machine execution."""
from pathlib import Path
import hashlib
import shutil
from PIL import Image
from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance, wait_for, private
from file_manager_phase4_acceptance import open_file, click_word


def pixels(session, shot):
    with Image.open(session.output/f'{shot}.png') as image:
        return image.crop((346,158,1182,580)).tobytes()


def exercise(session, root):
    original=hashlib.sha256((root/'frame.gcode').read_bytes()).digest()
    session.observe('01-strip',['frame.gcode','55 moves','program coordinates'])
    open_file(session,root,'frame.gcode','02-open')
    session.observe('03-path',['55 moves','Progress','Source line 15','Extruding','X 5.000','Z 0.400','Program coordinates'])
    baseline=pixels(session,'03-path')
    private.run_checked([str(session.pointer),'drag','1280','800','1050','350','1120','390'],env=session.env)
    session.observe('04-pan',['Source line 15'])
    assert pixels(session,'04-pan')!=baseline
    session.click(134,352)
    session.observe('05-fit',['Source line 15'])
    assert pixels(session,'05-fit')==baseline
    session.click(163,310)
    session.observe('06-projection',['55 moves','Source line 15'])
    assert pixels(session,'06-projection')!=baseline
    session.click(117,310)
    session.observe('07-xy',['55 moves'])
    assert pixels(session,'07-xy')==baseline
    # Edit the progress value, then choose the next genuine source move.
    session.click(226,226,delay=0.05)
    session.click(226,226,delay=0.05)
    session.key('a','ctrl')
    private.run_checked(['wtype','5'],env=session.env)
    session.key('Return')
    session.observe('08-scrub',['Source line 9','Z 0.200','Extruding'])
    assert pixels(session,'08-scrub')!=baseline
    session.click(630,684)
    session.observe('08b-source',['Source line 10','Z 0.400','Rapid travel'])
    session.click(205,126)
    session.observe('09-rename',['Filename'])
    session.key('a','ctrl')
    private.run_checked(['wtype','renamed.gcode'],env=session.env)
    session.key('Return')
    wait_for(lambda:(root/'renamed.gcode').exists(),'toolpath rename')
    assert hashlib.sha256((root/'renamed.gcode').read_bytes()).digest()==original
    session.observe('10-renamed',['renamed.gcode','55 moves'])
    session.click(1110,70)
    session.observe('11-folder',['renamed.gcode','55 moves','unsupported.gcode'])
    open_file(session,root,'unsupported.gcode','12-unsupported')
    session.observe('13-partial',['1 moves','Preview stops before line 2','unsupported G18'])
    session.click(1110,70)
    session.observe('14-returned',['renamed.gcode','notes.txt'])
    open_file(session,root,'renamed.gcode','15-reopen')
    session.observe('16-reopened',['55 moves','Source line 15'])
    session.click(1110,70)


def main():
    repo=Path(__file__).resolve().parents[1]
    args=application_arguments(__doc__,repo/'target/release/clip-file-manager')
    output,root=media_fixture_directory(args)
    shutil.copyfile(repo/'crates/little-clip-core/tests/fixtures/gcode/frame.gcode',root/'frame.gcode')
    (root/'unsupported.gcode').write_text('G1X10F300\nG18\nG1X999\n')
    (root/'notes.txt').write_text('An ordinary neighboring file.\n')
    return run_application_acceptance(args,output,root,exercise,'Toolpath folder previews, extrusion colors, pan/fit/projections, source selection, partial preview diagnostics and byte-preserving rename/reopen')


if __name__=='__main__':
    raise SystemExit(main())
