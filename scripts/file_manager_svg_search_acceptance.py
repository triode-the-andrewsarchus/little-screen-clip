#!/usr/bin/env python3
"""Actual SVG visual embedding and cross-modal retrieval in the private compositor."""
from pathlib import Path

from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance
from file_manager_maintenance_acceptance import search_environment
from file_manager_reuse_acceptance import inventory
from file_manager_search_acceptance import submit


def exercise(session, root):
    session.observe('initial', ['diagram.svg'])
    source = (root / 'diagram.svg').read_bytes()
    index = Path(session.env['XDG_DATA_HOME']) / 'little-screen-clip/search/index.sqlite'
    assert not index.exists()
    session.click(993, 118)
    session.key('Escape')
    _, paths, vectors = inventory(index)
    assert paths == [str(root / 'diagram.svg')] and vectors == 1
    submit(session, ':emb:"a blue circle on a white background"')
    session.observe('svg-visual-retrieval', ['diagram.svg', 'Semantic image match', 'static SVG image'])
    assert (root / 'diagram.svg').read_bytes() == source


def main():
    repository = Path(__file__).resolve().parent.parent
    args = application_arguments(__doc__, repository / 'target/debug/clip-file-manager')
    output, root = media_fixture_directory(args)
    (root / 'diagram.svg').write_text('<svg xmlns="http://www.w3.org/2000/svg" width="240" height="180"><circle cx="120" cy="80" r="60" fill="blue"/><text x="70" y="165" font-size="18" fill="black">Blue circle</text></svg>')
    return run_application_acceptance(args, output, root, exercise,
        'Source-preserving SVG raster preparation, one visual vector and semantic-only blue-circle retrieval with real model', search_environment)


if __name__ == '__main__':
    raise SystemExit(main())
