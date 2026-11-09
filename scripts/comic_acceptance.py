#!/usr/bin/env python3
"""Read comic archive pages through private Weston/Niri with source preservation."""
from pathlib import Path
import hashlib
import shutil
from PIL import Image
from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance, wait_for, private
from file_manager_phase4_acceptance import open_file, click_word


def exercise(session, root):
    original = hashlib.sha256((root / 'comic.cbz').read_bytes()).digest()
    session.observe('01-page-strip', ['comic.cbz', '3 pages'])
    _, y = session.word('01-page-strip', 'comic.cbz', minimum_y=240)
    session.click(760, y + 45, delay=0.05)
    session.click(760, y + 45, delay=0.05)
    session.observe('02-clicked-page', ['Page 2 / 3', 'page2.png', 'Zoom', 'Fit width', 'GREEN STORY'])
    full = green_pixels(session, '02-clicked-page')
    assert full > 20000, 'The selected green page was not rendered'
    # Zoom field and Fit width centers verified in this fixed private viewport.
    session.click(900, 176, delay=0.05)
    session.click(900, 176, delay=0.05)
    session.key('a', 'ctrl')
    private.run_checked(['wtype', '25'], env=session.env)
    session.key('Return')
    # Painted extents verify zoom; OCR can omit the small numeric field.
    session.observe('02-zoomed-out', ['GREEN STORY', 'Page 2 / 3'])
    small = green_pixels(session, '02-zoomed-out')
    assert 5000 < small < full * 0.5, (full, small)
    session.click(713, 176)
    session.observe('02-fit-width', ['GREEN STORY', 'Page 2 / 3'])
    assert green_pixels(session, '02-fit-width') > full * 0.8
    click_word(session, '02-clicked-page', 'Previous', minimum_y=160)
    session.observe('03-first-page', ['Page 1 / 3', 'page1.png', 'RED STORY'])
    click_word(session, '03-first-page', 'Next', minimum_y=160)
    session.observe('04-second-page', ['Page 2 / 3', 'page2.png', 'GREEN STORY'])
    click_word(session, '04-second-page', 'Next', minimum_y=160)
    session.observe('05-natural-order', ['Page 3 / 3', 'page10.png', 'BLUE STORY'])
    x,y = session.word('05-natural-order', 'comic.cbz', tree=True, minimum_y=90)
    session.click(x+10,y)
    session.observe('06-rename', ['Filename'])
    session.key('a','ctrl')
    private.run_checked(['wtype','renamed.cbz'],env=session.env)
    session.key('Return')
    wait_for(lambda: (root/'renamed.cbz').exists(), 'comic rename')
    assert hashlib.sha256((root/'renamed.cbz').read_bytes()).digest() == original
    session.observe('07-renamed', ['renamed.cbz','Page 3 / 3','page10.png'])
    click_word(session, '07-renamed', 'Previous', minimum_y=160)
    session.observe('08-read-after-rename', ['Page 2 / 3','page2.png','GREEN STORY'])
    session.click(1110,70)
    session.observe('09-folder', ['renamed.cbz','3 pages','notes.txt'])
    open_file(session,root,'renamed.cbz','10-reopen')
    session.observe('11-reopened', ['Page 1 / 3','page1.png','RED STORY'])
    session.click(1110,70)
    session.observe('12-returned', ['renamed.cbz','notes.txt'])


def green_pixels(session, shot):
    with Image.open(session.output / f'{shot}.png') as image:
        return sum(g-r > 80 and g-b > 60 for r,g,b in image.convert('RGB').getdata())


def main():
    repo=Path(__file__).resolve().parents[1]
    args=application_arguments(__doc__,repo/'target/release/clip-file-manager')
    output,root=media_fixture_directory(args)
    shutil.copyfile(repo/'crates/clip-workbench/tests/fixtures/comic.cbz',root/'comic.cbz')
    (root/'notes.txt').write_text('Neighboring file.\n')
    return run_application_acceptance(args,output,root,exercise,'comic page strip, natural order, zoom/fit-width pixel extents, guarded rename and pinned reads after rename')


if __name__=='__main__':
    raise SystemExit(main())
