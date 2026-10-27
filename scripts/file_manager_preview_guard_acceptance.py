#!/usr/bin/env python3
"""Passive result previews reject a changed offscreen source before payload reads."""
import json
from pathlib import Path
import re

from PIL import Image

from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance, wait_for
from file_manager_offline_acceptance import chunks, trace_application
from file_manager_search_acceptance import submit


def reads(session, path):
    return [line for text in chunks(session).values() for line in text.splitlines()
            if str(path) + '>' in line and re.search(r'\b(?:read|pread64)\(', line)]


def exercise(session, root):
    session.observe('initial', ['Nested'])
    target = root / 'Nested/needle_99.txt'
    index = Path(session.env['XDG_DATA_HOME']) / 'little-screen-clip/search/index.sqlite'
    assert not index.exists()
    submit(session, ':*.txt needle;scope:recursive')
    session.observe('current-previews', ['needle_00.txt', '13 matching entries', '1 lines'])
    assert not reads(session, target), 'The stale target must start outside the preview viewport'
    offsets = {path.name: path.stat().st_size for path in session.output.glob('source-trace.*')}
    target.write_text('Replacement payload must wait for an explicit refresh.\n' * 100)
    session.scroll_to_visible('stale-preview', ['needle_99.txt'], step=300, position=(1050, 450))
    denied_reads = reads(session, target)
    assert not denied_reads, denied_reads
    # Verify the trace actually covers this source's metadata admission attempt.
    def observations():
        return [line for text in chunks(session, offsets).values() for line in text.splitlines()
                if str(target) in line and ('openat(' in line or 'openat2(' in line)]
    wait_for(lambda: observations(), 'the stale preview attempts source validation')
    observed = observations()
    assert not reads(session, target), 'Stale validation must not decode replacement bytes'
    session.key('F5')
    session.scroll_to_visible('refreshed-preview', ['needle_99.txt', '100 lines'], step=300, position=(1050, 450))
    fresh_reads = reads(session, target)
    assert fresh_reads, 'Refresh must admit the new source and decode its preview'
    submit(session, ':*.png picture;scope:recursive')
    session.observe('image-result', ['picture.png', '1 matching entries'])
    session.click(*session.word('image-result', 'Photos', tree=True))
    photo = session.output / 'guarded-photo.png'
    def visible_photo():
        session.capture_text('guarded-photo', photo, [])
        with Image.open(photo) as image:
            return sum(abs(r - 9) < 8 and abs(g - 170) < 8 and abs(b - 70) < 8
                       for r, g, b in image.convert('RGB').crop((350, 300, 1180, 550)).getdata()) > 1000
    wait_for(visible_photo, 'guarded Photos thumbnail and metadata finish loading')
    assert reads(session, root / 'Nested/picture.png'), 'The valid image preview must actually read its source'
    assert not index.exists(), 'Passive filename previews must not enroll a folder'
    (session.output / 'preview-guard.json').write_text(json.dumps({
        'stale_payload_reads': len(denied_reads), 'fresh_payload_reads': len(fresh_reads),
        'metadata_observations': len(observed), 'explicit_refresh_required': True,
        'no_index_created': True, 'guarded_photo_rendered': True,
    }, indent=2))


def main():
    repository = Path(__file__).resolve().parent.parent
    args = application_arguments(__doc__, repository / 'target/debug/clip-file-manager')
    output, root = media_fixture_directory(args)
    nested = root / 'Nested'
    nested.mkdir()
    for index in [*range(12), 99]:
        (nested / f'needle_{index:02}.txt').write_text('Original preview content.\n')
    Image.new('RGB', (320, 200), (9, 170, 70)).save(nested / 'picture.png')
    trace_application(args, output)
    return run_application_acceptance(args, output, root, exercise,
        'Live passive previews retain source observations, reject stale offscreen content, and recover after explicit refresh without enrollment')


if __name__ == '__main__':
    raise SystemExit(main())
