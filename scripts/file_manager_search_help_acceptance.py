#!/usr/bin/env python3
"""Context help and question-format completion on private Weston/Niri only."""
from pathlib import Path
import subprocess

from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance
from file_manager_search_controls_acceptance import type_text


def draft(session, text):
    session.key('f', 'ctrl')
    type_text(session, text)


def field_text(session):
    session.key('a', 'ctrl')
    session.key('c', 'ctrl')
    # Capture before Niri's screenshot action replaces the private clipboard.
    result = subprocess.run(['wl-paste', '--no-newline', '--type', 'text/plain'],
        env=session.env, capture_output=True, timeout=5, check=True).stdout.decode()
    session.key('End')
    return result


def exercise(session, root):
    session.observe('initial', ['Alpha.md', 'Other.txt'])
    draft(session, ':im')
    session.observe('selector-suggestion', ['Tab completes', 'Alpha.md', 'Other.txt'])
    session.key('Tab')
    assert field_text(session) == ':image'
    # Completion is draft-only: the text file remains in the untouched listing.
    session.observe('selector-completed-no-submit', ['Other.txt', 'Alpha.md'])

    draft(session, ':image;scope:')
    session.observe('scope-values', ['recursive', 'indexed'])
    session.key('Down')
    session.key('Tab')
    assert field_text(session) == ':image;scope:recursive'
    session.observe('scope-completed', ['scope:recursive', 'Other.txt'])

    draft(session, r':*.md;q:"Date: yyyy-mm-dd; as date, \"quoted\"')
    session.observe('question-text-help', ['Question text', 'Other.txt'])
    draft(session, ':regex:"(?i)foo;bar,\\d+')
    session.observe('regex-text-help', ['Regular expression', 'filename start'])
    draft(session, ':content:"literal; text')
    session.observe('content-text-help', ['Content text', 'literal phrase'])
    draft(session, ':emb:"red bicycle')
    session.observe('meaning-text-help', ['Meaning', 'indexed folders'])

    question = ':*.md;q:"What is the date in the header?" as '
    draft(session, question)
    session.observe('answer-formats', ['Answer format', 'text', 'number', 'date', 'boolean'])
    session.key('Down')
    session.key('Down')
    session.key('Tab')
    assert field_text(session) == question + 'date'
    session.observe('date-format-completed', ['as date', 'Other.txt'])

    draft(session, question + 'b')
    session.observe('boolean-suggestion', ['boolean', 'Tab completes'])
    # OCR can omit black text on the highlighted blue suggestion. Its first
    # row sits immediately below the keyboard hint on this private fixed output.
    _, y = session.word('boolean-suggestion', 'Tab', tree=True)
    session.click(160, y + 34)
    assert field_text(session) == question + 'boolean'
    session.observe('clicked-format', ['as boolean', 'Other.txt'])

    draft(session, ':image;scope:rec;sort:name')
    for _ in ';sort:name':
        session.key('Left')
    session.key('Tab')
    assert field_text(session) == ':image;scope:recursive;sort:name'
    session.observe('middle-completion', ['scope:recursive;sort:name'])

    draft(session, ':*.md;q:"Date?" as unknown')
    session.key('Return')
    session.observe('invalid-format-retains-listing', ['Alpha.md', 'Other.txt', 'Answer type must'])
    draft(session, ':*.md')
    session.key('Return')
    session.observe('explicit-submit', ['Alpha.md', '1 matching entries'])
    session.key('Escape')
    session.observe('restored', ['Alpha.md', 'Other.txt'])
    search = Path(session.env['XDG_DATA_HOME']) / 'little-screen-clip/search'
    assert not (search / 'index.sqlite').exists()
    assert not (search / 'models').exists()
    assert not (search / 'runtime').exists()


def main():
    repo = Path(__file__).resolve().parent.parent
    args = application_arguments(__doc__, repo / 'target/debug/clip-file-manager')
    output, root = media_fixture_directory(args)
    (root / 'Alpha.md').write_text('---\ndate: 2026-11-02\n---\nAn ordinary post.\n')
    (root / 'Other.txt').write_text('A plain file retained while editing drafts.')
    return run_application_acceptance(args, output, root, exercise,
        'Caret-aware command help, open payload descriptions, keyboard/click completion, trailing typed questions, draft-only changes, explicit submission, focus and suffix preservation without index/models')


if __name__ == '__main__':
    raise SystemExit(main())
