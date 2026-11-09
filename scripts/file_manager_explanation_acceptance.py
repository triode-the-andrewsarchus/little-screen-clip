#!/usr/bin/env python3
"""Search excerpts, provenance and row interaction on the private compositor."""
from pathlib import Path
import hashlib

import ui_acceptance_capture as private
from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance
from file_manager_reuse_acceptance import inventory
from file_manager_search_acceptance import submit


def exercise(session, root):
    sources = {path: hashlib.sha256(path.read_bytes()).hexdigest() for path in root.rglob('*.txt')}
    index = Path(session.env['XDG_DATA_HOME']) / 'little-screen-clip/search/index.sqlite'
    session.observe('initial', ['Notes', 'Other', 'Shelf (0)'])
    session.click(530, 160)  # Shelf toolbar; OCR also finds Shelf in collection cards.
    submit(session, ':content:"violet lighthouse";scope:recursive')
    session.observe('literal-explanations', ['record.txt', 'Content match', 'Violet lighthouse', '/Notes', '/Other'])
    assert not index.exists(), 'Explaining a content search must not create an index'
    x, y = session.word('literal-explanations', 'Violet', minimum_y=190)
    session.click(x + 5, y)
    session.key('c', 'ctrl')
    payload = private.run_checked(['wl-paste', '--no-newline', '--type', 'text/uri-list'], env=session.env)
    assert payload.strip() in {path.as_uri() for path in sources}, payload
    (session.output / 'footer-clipboard.txt').write_text(payload)
    session.key('Escape')
    session.observe('restored', ['Notes', 'Other'])
    session.click(993, 118)
    session.key('Escape')
    _, paths, vectors = inventory(index)
    assert len(paths) == 2 and vectors >= 2, (paths, vectors)
    submit(session, ':emb:"violet lighthouse";scope:recursive')
    session.observe('indexed-explanations', ['record.txt', 'Keyword match', 'Violet lighthouse', '/Notes', '/Other'])
    for path, checksum in sources.items():
        assert hashlib.sha256(path.read_bytes()).hexdigest() == checksum


def main():
    repository = Path(__file__).resolve().parent.parent
    args = application_arguments(__doc__, repository / 'target/debug/clip-file-manager')
    output, root = media_fixture_directory(args)
    for directory, ending in [('Notes', 'guides fishing boats.'), ('Other', 'stands beside the harbour.')]:
        (root / directory).mkdir()
        (root / directory / 'record.txt').write_text('Ordinary background context. ' * 80 + f'Violet lighthouse {ending}')

    def environment(env, children):
        data = Path(env['XDG_DATA_HOME']) / 'little-screen-clip/search'
        data.mkdir(parents=True)
        for directory in ['runtime', 'models']:
            (data / directory).symlink_to(Path.home() / '.local/share/little-screen-clip/search' / directory, target_is_directory=True)

    return run_application_acceptance(args, output, root, exercise,
        'Literal and indexed excerpts, parent provenance, footer selection/copy, no implicit enrollment or source changes', environment)


if __name__ == '__main__':
    raise SystemExit(main())
