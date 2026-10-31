#!/usr/bin/env python3
"""Automatic indexed directory moves/replacement in private Weston/Niri."""
import json
from pathlib import Path
import shutil

from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance
from file_manager_maintenance_acceptance import search_environment
from file_manager_membership_acceptance import expect_catalog
from file_manager_search_acceptance import submit


def settled(session, root, index, label, paths, previous=None):
    return expect_catalog(session, index, label, lambda c:
        set(c['roots']) == {str(root)} and c['roots'][str(root)]['state'] == 'ready'
        and set(c['paths']) == {str(path) for path in paths}
        and (previous is None or c['roots'][str(root)]['generation'] > previous['roots'][str(root)]['generation']))


def generated(session, label, count):
    session.click(993, 118)
    session.observe(label, [f'{count} generated'])
    session.key('Escape')


def replace_child(session, root, index, previous):
    quiet = root / 'Quiet/Other.txt'
    old = root / 'Renamed/One.txt'
    stage = session.output / 'replacement'
    stage.mkdir()
    (stage / 'New.txt').write_text('Replacement sentinel: nocturnal bats navigate by echolocation.\n')
    (root / 'Renamed').rename(session.output / 'displaced-child')
    stage.rename(root / 'Renamed')
    state = settled(session, root, index, 'replacement-discovered', [quiet, root / 'Renamed/New.txt'], previous)
    assert str(old) not in state['paths'] and len(state['vectors']) == 2
    quiet_id = previous['paths'][str(quiet)]
    assert state['paths'][str(quiet)] == quiet_id
    assert [v for v in state['vectors'] if v['file'] == quiet_id] == [v for v in previous['vectors'] if v['file'] == quiet_id]
    (root / 'Renamed/New.txt').write_text('Edited replacement sentinel: honey bees pollinate flowering plants.\n')
    edited = expect_catalog(session, index, 'replacement-watch-edited', lambda c:
        c['roots'][str(root)]['state'] == 'ready'
        and c['roots'][str(root)]['generation'] > state['roots'][str(root)]['generation']
        and any('honey bees' in v['text'] for v in c['vectors']))
    assert len(edited['paths']) == len(edited['vectors']) == 2
    return edited


def nested_moves(session, root, index, previous):
    stage = session.output / 'incoming'
    (stage / 'Nested').mkdir(parents=True)
    (stage / 'Nested/Deep.txt').write_text('A deep sea octopus explores the ocean floor.\n')
    stage.rename(root / 'Incoming')
    quiet, new = root / 'Quiet/Other.txt', root / 'Renamed/New.txt'
    before = settled(session, root, index, 'nested-import', [quiet, new, root / 'Incoming/Nested/Deep.txt'], previous)
    assert len(before['vectors']) == 3
    generated(session, 'before-nested-move', 5)
    (root / 'Incoming').rename(root / 'Renamed/Imported')
    after = settled(session, root, index, 'nested-move', [quiet, new, root / 'Renamed/Imported/Nested/Deep.txt'], before)
    assert after['vectors'] == before['vectors'], 'Moving a directory must preserve canonical vectors'
    generated(session, 'nested-move-reuses-embeddings', 5)
    shutil.rmtree(root / 'Renamed/Imported')
    deleted = settled(session, root, index, 'nested-removal', [quiet, new], after)
    assert len(deleted['vectors']) == 2
    return deleted


def replace_root(session, root, index, previous):
    root.rename(session.output / 'displaced-root')
    root.mkdir()
    (root / 'NewRoot.txt').write_text('A different directory must never inherit the prior enrollment.\n')
    offline = expect_catalog(session, index, 'root-replacement-offline', lambda c:
        set(c['roots']) == {str(root)} and c['roots'][str(root)]['state'] == 'offline')
    assert offline['paths'] == previous['paths'] and offline['vectors'] == previous['vectors']
    assert str(root / 'NewRoot.txt') not in offline['paths']
    session.address(root)
    session.observe('replacement-root-browser', ['NewRoot.txt'])
    session.click(993, 118)
    session.observe('replacement-root-needs-enrollment', ['identity changed', 'Remove indexed folder'])
    session.key('Escape')


def exercise(session, root):
    index = Path(session.env['XDG_DATA_HOME']) / 'little-screen-clip/search/index.sqlite'
    session.observe('initial', ['Alpha', 'Quiet'])
    assert not index.exists()
    session.click(993, 118)
    session.key('Escape')
    initial = settled(session, root, index, 'initial-indexed', [root / 'Alpha/One.txt', root / 'Quiet/Other.txt'])
    assert len(initial['vectors']) == 2
    (root / 'Alpha').rename(root / 'Renamed')
    moved = settled(session, root, index, 'directory-renamed', [root / 'Renamed/One.txt', root / 'Quiet/Other.txt'], initial)
    assert moved['vectors'] == initial['vectors']
    generated(session, 'directory-move-reuses-embeddings', 2)
    replaced = replace_child(session, root, index, moved)
    final = nested_moves(session, root, index, replaced)
    submit(session, ':regex:^New[.]txt$;scope:indexed')
    session.observe('new-path-searchable', ['New.txt', '1 matching entries'])
    submit(session, ':regex:^One[.]txt$;scope:indexed')
    session.observe('retired-path-not-searchable', ['No matching files'])
    session.key('Escape')
    assert (session.output / 'displaced-child/One.txt').read_text() == 'A red fox lives in a temperate forest.\n'
    assert (root / 'Quiet/Other.txt').read_text() == 'A telescope records distant stars and galaxies.\n'
    replace_root(session, root, index, final)
    (session.output / 'directory-checks.json').write_text(json.dumps(dict(
        automatic_directory_rename=True, same_path_child_replacement=True,
        replacement_watch_edit=True, nested_import_move_removal=True,
        moved_vectors_unchanged=True, moved_generated_count_unchanged=True,
        stale_path_search_absent=True, replacement_root_requires_enrollment=True,
        manual_rebuilds=0), indent=2))


def main():
    repo = Path(__file__).resolve().parent.parent
    args = application_arguments(__doc__, repo / 'target/release/clip-file-manager')
    output, root = media_fixture_directory(args)
    for name in ['Alpha', 'Quiet']:
        (root / name).mkdir()
    (root / 'Alpha/One.txt').write_text('A red fox lives in a temperate forest.\n')
    (root / 'Quiet/Other.txt').write_text('A telescope records distant stars and galaxies.\n')
    return run_application_acceptance(args, output, root, exercise,
        'Automatic directory rename/replacement, nested moves, canonical reuse and replaced-root rejection without manual rebuild', search_environment)


if __name__ == '__main__':
    raise SystemExit(main())
