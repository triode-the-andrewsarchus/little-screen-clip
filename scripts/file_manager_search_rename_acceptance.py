#!/usr/bin/env python3
"""Search-result renames retain their original source and actual parent folder."""
import json
from pathlib import Path

from file_manager_acceptance import (
    application_arguments, media_fixture_directory, run_application_acceptance, wait_for,
)
from file_manager_search_acceptance import submit
from file_manager_search_controls_acceptance import type_text


def rename_dialog(session, root, label):
    session.address(root)
    submit(session, ':*.txt;scope:recursive')
    session.observe(label + '-result', ['1 matching entries', 'target.txt'])
    session.click(*session.word(label + '-result', 'target.txt', minimum_y=250))
    session.key('F2')
    session.observe(label + '-dialog', ['Rename', 'target.txt'])
    type_text(session, 'renamed.txt')


def exercise(session, root):
    nested = root/'Nested'
    source, destination = nested/'target.txt', nested/'renamed.txt'
    session.observe('initial', ['Nested'])
    rename_dialog(session, root, 'valid')
    session.key('Return')
    wait_for(destination.exists, 'valid nested search result is renamed in its own folder')
    assert not source.exists() and destination.read_bytes() == b'original selected bytes\n'
    assert not (root/'renamed.txt').exists()
    # Recreate a fresh search observation, then change the selected source after
    # the dialog captured it. A retry must retain that observation too.
    destination.rename(source)
    rename_dialog(session, root, 'replaced')
    source.rename(nested/'saved-original.txt')
    source.write_bytes(b'replacement must remain untouched\n')
    session.key('Return')
    session.observe('replacement-rejected', ['Rename failed', 'Search result changed'])
    assert source.read_bytes() == b'replacement must remain untouched\n'
    assert not destination.exists()
    session.key('Return')
    session.observe('retry-still-rejected', ['Rename failed', 'Search result changed'])
    assert source.read_bytes() == b'replacement must remain untouched\n'
    assert not destination.exists()
    assert (nested/'saved-original.txt').read_bytes() == b'original selected bytes\n'
    index = Path(session.env['XDG_DATA_HOME'])/'little-screen-clip/search/index.sqlite'
    assert not index.exists()
    (session.output/'rename-checks.json').write_text(json.dumps(dict(
        nested_parent_preserved=True, valid_rename_preserves_bytes=True,
        replacement_rejected=True, retry_preserves_original_guard=True,
        replacement_bytes_unchanged=True, no_index_created=True), indent=2))


def main():
    repo = Path(__file__).resolve().parent.parent
    args = application_arguments(__doc__, repo/'target/release/clip-file-manager')
    output, root = media_fixture_directory(args)
    (root/'Nested').mkdir()
    (root/'Nested/target.txt').write_bytes(b'original selected bytes\n')
    return run_application_acceptance(args, output, root, exercise,
        'Nested search rename targets its actual parent; source replacement after dialog admission and retry cannot rename the replacement; no implicit index')


if __name__ == '__main__':
    raise SystemExit(main())
