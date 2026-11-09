#!/usr/bin/env python3
"""Search-origin drag retains source continuity through a destination conflict."""
import json
from pathlib import Path

import ui_acceptance_capture as private
from file_manager_acceptance import (
    application_arguments, media_fixture_directory, run_application_acceptance,
)
from file_manager_search_acceptance import submit


def exercise(session, root, stale):
    session.observe('initial', ['Source', 'ZDestination'])
    source = root/'Source'
    target = root/'ZDestination/target.txt'
    session.address(source)
    submit(session, ':*.txt;scope:recursive')
    session.observe('source-result', ['1 matching entries', 'target.txt', 'ZDestination'])
    start = session.word('source-result', 'target.txt', minimum_y=250)
    end = session.word('source-result', 'ZDestination', tree=True, minimum_y=250)
    session.click(*start)
    private.run_checked([str(session.pointer), 'drag', '1280', '800',
                         *map(str, (*start, *end))], env=session.env)
    session.observe('conflict', ['Destination already exists', 'Replace', 'Skip'])
    original = root/'Source-original'
    leaf_before = (source/'target.txt').stat()
    if stale:
        source.rename(original)
        source.symlink_to(original, target_is_directory=True)
        leaf_after = (source/'target.txt').stat()
        assert (leaf_before.st_dev, leaf_before.st_ino, leaf_before.st_size,
                leaf_before.st_mtime_ns, leaf_before.st_ctime_ns) == (
                    leaf_after.st_dev, leaf_after.st_ino, leaf_after.st_size,
                    leaf_after.st_mtime_ns, leaf_after.st_ctime_ns)
    anchor = session.word('conflict', 'Apply', minimum_y=400)
    session.click(anchor[0] + 15, anchor[1] + 42)
    session.scroll_to_visible('transfer-report', ['Move:', 'completed'], step=-80,
                              attempts=5, position=(700, 530))
    state = dict(stale=stale, source_present=(source/'target.txt').exists(),
                 destination_bytes=target.read_text())
    (session.output/'drag-state.json').write_text(json.dumps(state, indent=2))
    if stale:
        assert target.read_bytes() == b'old destination bytes\n', state
        assert (original/'target.txt').read_bytes() == b'original selected bytes\n', state
        session.scroll_to_visible('rejected-header', ['Move: 0 completed', '1 failed'],
                                  step=-80, attempts=5, position=(700, 530))
        session.scroll_to_visible('rejected-reason', ['Search result changed'],
                                  step=60, attempts=5, position=(700, 530))
        source.unlink()
        original.rename(source)
    else:
        assert not (source/'target.txt').exists(), state
        assert target.read_bytes() == b'original selected bytes\n', state
        session.observe('valid-move-completed', ['Move: 1 completed', '0 failed'])
    index = Path(session.env['XDG_DATA_HOME'])/'little-screen-clip/search/index.sqlite'
    assert not index.exists()
    (session.output/'drag-checks.json').write_text(json.dumps(dict(
        stale_root_rejected=stale, valid_move_completed=not stale, no_implicit_index=True,
    ), indent=2))


def main():
    repo = Path(__file__).resolve().parent.parent
    args = application_arguments(__doc__, repo/'target/release/clip-file-manager',
                                 lambda parser: parser.add_argument('--stale-root', action='store_true'))
    output, root = media_fixture_directory(args)
    for name, payload in [('Source', b'original selected bytes\n'),
                          ('ZDestination', b'old destination bytes\n')]:
        folder = root/name
        folder.mkdir()
        (folder/'target.txt').write_bytes(payload)
    return run_application_acceptance(args, output, root,
        lambda session, folder: exercise(session, folder, args.stale_root),
        'Search-origin native self-drop preserves source context through Replace; valid move succeeds or an unchanged leaf under an aliased root is rejected')


if __name__ == '__main__':
    raise SystemExit(main())
