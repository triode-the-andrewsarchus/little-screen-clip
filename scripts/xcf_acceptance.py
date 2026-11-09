#!/usr/bin/env python3
"""Inspect and export a native XCF canvas in Files on private Weston/Niri."""
from pathlib import Path
import hashlib
import shutil
from PIL import Image
from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance, wait_for, private
from file_manager_phase4_acceptance import open_file
from image_collection_acceptance import include_strip_ocr, click_word, click_control, flip_photo, observe_badge


def exercise(session, root):
    original=hashlib.sha256((root/'layered.xcf').read_bytes()).digest()
    include_strip_ocr(session,source_layers=True)
    session.observe('01-native-row',['layered.xcf','96','64'])
    open_file(session,root,'layered.xcf','02-open')
    text=session.observe('03-source',['Rendered canvas','Source layers','Hidden red','Panel group','Blue panel','Paper'])
    assert 'unsaved' not in text.lower()
    # Source-layer row center is resolved from the private sidebar OCR.
    x,y=session.word('03-source','panel',tree=True,minimum_y=350,last=True)
    session.click(x+8,y)
    text=session.observe('04-layer',['Raster mask','Visible','100%','Normal'])
    assert 'unsaved' not in text.lower()
    # Source layers are an inspection; choosing one must not replace the rendered canvas.
    with Image.open(session.output/'04-layer.png') as image:
        pixels=list(image.convert('RGB').crop((350,310,1170,710)).getdata())
        assert sum(b>170 and b>r+80 and b>g+60 for r,g,b in pixels)>500,'Native blue canvas region missing'
    flip_photo(session,'05-transform')
    observe_badge(session,'06-edited',['Export'])
    session.click(220,230)
    session.observe('07-rename',['Filename'])
    session.key('a','ctrl')
    private.run_checked(['wtype','renamed.xcf'],env=session.env)
    session.key('Return')
    wait_for(lambda:(root/'renamed.xcf').exists(),'XCF rename')
    observe_badge(session,'08-renamed',['renamed.xcf'])
    click_control(session,'08-renamed','Export')
    session.observe('09-export',['Output folder','Export PNG copies'])
    session.click(540,525)
    output=root/'Edited'/'renamed_edit.png'
    wait_for(output.exists,'XCF raster export')
    session.observe('10-exported',['Export all edits','renamed.xcf'])
    with Image.open(output) as exported:
        assert exported.size==(96,64)
        pixel=exported.convert('RGBA').getpixel((30,24))
        assert pixel[2]>200 and pixel[0]<40 and pixel[1]<120,pixel
    assert hashlib.sha256((root/'renamed.xcf').read_bytes()).digest()==original
    click_control(session,'10-exported','Back')
    session.observe('11-folder',['renamed.xcf','notes.txt'])
    open_file(session,root,'renamed.xcf','12-reopen')
    session.observe('13-reopened',['Rendered canvas','Source layers','Blue panel'])
    click_control(session,'13-reopened','Back')
    session.observe('14-returned',['renamed.xcf','notes.txt'])


def main():
    repo=Path(__file__).resolve().parents[1]
    args=application_arguments(__doc__,repo/'target/release/clip-file-manager')
    output,root=media_fixture_directory(args)
    shutil.copyfile(repo/'crates/little-clip-core/tests/fixtures/layered.xcf',root/'layered.xcf')
    (root/'notes.txt').write_text('An ordinary neighboring file.\n')
    return run_application_acceptance(args,output,root,exercise,'native XCF canvas, source group/mask inspection, raster edit, guarded rename, PNG export preserving the layered original and reopen')


if __name__=='__main__':
    raise SystemExit(main())
