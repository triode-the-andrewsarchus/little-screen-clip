#!/usr/bin/env python3
"""Confirmed search mutations retain root identity as well as leaf identity."""
import json
from pathlib import Path

from file_manager_acceptance import (
    application_arguments, media_fixture_directory, run_application_acceptance,
    wait_for,
)
from file_manager_search_acceptance import submit


def confirm(session, folder, label, permanent):
    session.address(folder)
    submit(session, ':*.txt;scope:recursive')
    session.observe(label + '-result', ['1 matching entries', 'target.txt'])
    session.click(*session.word(label + '-result', 'target.txt', minimum_y=250))
    session.key('Delete', 'shift' if permanent else None)
    session.observe(label + '-confirmation', [
        'Delete permanently?' if permanent else 'Move to Trash?', 'Enter confirms',
    ])


def exercise(session, root):
    session.observe('initial', ['Valid', 'StaleDelete', 'StaleTrash'])
    folder = root/'Valid'
    confirm(session, folder, 'valid', True)
    session.key('Return')
    wait_for(lambda: not (folder/'target.txt').exists(), 'valid guarded deletion completes')
    session.observe('valid-completed', ['1 completed', '0 failed'])
    for name, permanent in [('StaleDelete', True), ('StaleTrash', False)]:
        folder = root/name
        source = folder/'target.txt'
        confirm(session, folder, name, permanent)
        root_before = folder.lstat()
        before = source.stat()
        original = root/(name + '-original')
        folder.rename(original)
        folder.symlink_to(original, target_is_directory=True)
        after = source.stat()
        assert (before.st_dev, before.st_ino, before.st_size, before.st_mtime_ns, before.st_ctime_ns) == (
            after.st_dev, after.st_ino, after.st_size, after.st_mtime_ns, after.st_ctime_ns,
        ), 'the leaf must be unchanged; only original root continuity is lost'
        session.key('Return')
        try:
            title = 'Permanent deletion' if permanent else 'Trash'
            position = (700, 530)
            session.scroll_to_visible(name + '-report', [title + ': 0 completed', '1 failed'],
                                      step=-80, attempts=4, position=position)
            # Report index 0 can retain the prior report's collapse preference.
            # Its bounded status dock also needs scrolling to expose details.
            for attempt in range(2):
                try:
                    session.scroll_to_visible(name + '-rejected', ['Search result changed'],
                                              step=60, attempts=4, position=position)
                    break
                except RuntimeError:
                    if attempt:
                        raise
                    session.scroll_to_visible(name + '-report-top', [title + ': 0 completed'],
                                              step=-80, attempts=4, position=position)
                    session.click(*session.word(name + '-report-top',
                                                'Permanent' if permanent else 'Trash:',
                                                tree=True, minimum_y=300))
        finally:
            (session.output/(name + '-state.json')).write_text(json.dumps(dict(
                source_present=source.exists(), original_present=(original/'target.txt').exists(),
                root_is_alias=folder.is_symlink(), original_root_inode=root_before.st_ino,
                alias_root_inode=folder.lstat().st_ino, original_leaf_inode=before.st_ino,
                alias_leaf_inode=after.st_ino,
            ), indent=2))
        assert source.read_bytes() == (original/'target.txt').read_bytes() == b'preserve selected bytes\n'
        folder.unlink()
        original.rename(folder)
    index = Path(session.env['XDG_DATA_HOME'])/'little-screen-clip/search/index.sqlite'
    assert not index.exists(), 'filename result actions must not implicitly enroll a folder'
    (session.output/'mutation-checks.json').write_text(json.dumps(dict(
        valid_search_delete=True, stale_delete_rejected=True, stale_trash_rejected=True,
        unchanged_leaf_identity=True, original_payloads_preserved=True, no_index_created=True,
    ), indent=2))


def main():
    repo = Path(__file__).resolve().parent.parent
    args = application_arguments(__doc__, repo/'target/release/clip-file-manager')
    output, root = media_fixture_directory(args)
    for name in ['Valid', 'StaleDelete', 'StaleTrash']:
        folder = root/name
        folder.mkdir()
        (folder/'target.txt').write_bytes(b'preserve selected bytes\n')
    return run_application_acceptance(args, output, root, exercise,
        'Valid guarded permanent deletion; root-alias replacement after confirmation cannot delete or trash an unchanged observed leaf; zero implicit index')


if __name__ == '__main__':
    raise SystemExit(main())
