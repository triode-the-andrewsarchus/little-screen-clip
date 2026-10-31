#!/usr/bin/env python3
"""Check per-file applicability for equal front-matter layouts in private Files."""
import hashlib
from pathlib import Path

from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance
from file_manager_answer_review_acceptance import completed
from file_manager_frontmatter_acceptance import environment
from file_manager_search_acceptance import submit
from text_strip_acceptance import hover_observation


def exercise(session, root):
    before = {path.name:hashlib.sha256(path.read_bytes()).digest() for path in root.glob('*.md')}
    session.observe('initial', ['A-Contract.md', 'Z-Recipe.md'])
    session.click(647, 160)
    submit(session, ':q:"Does this rental agreement permit pets?";type:boolean')
    completed(session, 2)
    session.observe('answers', ['A-Contract.md', 'Z-Recipe.md', 'true', 'Session answers'], timeout_seconds=90)
    header_x, _ = session.word('answers', 'Does', minimum_y=220)
    _, recipe_y = session.word('answers', 'Z-Recipe.md', minimum_y=280)
    hover_observation(session, 'recipe-applicability', header_x + 30, recipe_y,
                      ['Not applicable to this question'])
    assert before == {path.name:hashlib.sha256(path.read_bytes()).digest() for path in root.glob('*.md')}
    assert not (Path(session.env['XDG_DATA_HOME'])/'little-screen-clip/search/index.sqlite').exists()


def main():
    repo = Path(__file__).resolve().parent.parent
    args = application_arguments(__doc__, repo/'target/debug/clip-file-manager')
    output, root = media_fixture_directory(args)
    (root/'A-Contract.md').write_text('---\ndocument: Rental agreement\npets_permitted: true\ntenant: Alice\n---\nThis is a rental agreement.\n')
    (root/'Z-Recipe.md').write_text('---\ndocument: Recipe\npets_permitted: true\ntenant: Bob\n---\nThis is a cooking recipe.\n')
    return run_application_acceptance(args, output, root, exercise,
        'Conditional boolean applicability across identical front-matter layouts without indexing', environment)


if __name__ == '__main__':
    raise SystemExit(main())
