#!/usr/bin/env python3
"""Literal reuse, per-file interpretation and semantic escalation in private Files."""
import hashlib
import json
import re
from pathlib import Path
from PIL import Image
import ui_acceptance_capture as private

from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance, wait_for
from file_manager_frontmatter_acceptance import environment
from file_manager_search_acceptance import submit


def completed(session, count):
    wait_for(lambda: (session.output/'files.log').read_text().count('column answer ready') >= count,
             f'{count} completed review-column cells', 90)
    return (session.output/'files.log').read_text()


def exercise(session, root):
    before = {str(path.relative_to(root)):hashlib.sha256(path.read_bytes()).hexdigest() for path in root.rglob('*.md')}
    session.observe('initial', ['Alpha.md', 'Beta.md'])
    session.click(647, 160)
    submit(session, ':q:"Does the summary sound positive?";type:boolean')
    interpreted = completed(session, 2)
    session.observe('interpretation-ready', ['true', 'false', 'Alpha.md', 'Beta.md'])
    assert not re.search(r'column candidate review.*action="?reuse', interpreted), 'A judgment must not authorize literal reuse'
    first_true = session.word('interpretation-ready', 'true', minimum_y=280)
    second_false = session.word('interpretation-ready', 'false', minimum_y=280)
    assert first_true[1] < second_false[1], (first_true, second_false)
    submit(session, ':q:"What is the stored positive flag?";type:boolean')
    literal = completed(session, 4)[len(interpreted):]
    session.observe('literal-ready', ['true', 'false', 'Session answers'])
    assert re.search(r'column candidate review.*action="?reuse', literal), literal
    assert literal.count('structured=true') == 2, literal
    last_true = session.word('literal-ready', 'true', minimum_y=280, last=True)
    assert last_true[0] > second_false[0] and abs(last_true[1]-second_false[1]) < 20, (last_true, second_false)
    before_age = (session.output/'files.log').read_text()
    session.address(root/'Escalation')
    submit(session, ':q:"How old is the child?";type:number')
    escalated = completed(session, 6)[len(before_age):]
    session.observe('age-corrected', ['Alice.md', 'Session answers'])
    assert 'structured=false' in escalated
    assert not re.search(r'column candidate review.*action="?reuse', escalated), 'A plausible first age cannot authorize later values without review'
    header = session.word('age-corrected', 'How', minimum_y=220)
    size = session.word('age-corrected', 'Size', minimum_y=220)
    # Isolate the answer column: ordinary row metadata can also contain a
    # standalone 6 (for example a line count) outside that column.
    crop = session.output/'age-values.png'
    with Image.open(session.output/'age-corrected.png') as screen:
        values = screen.crop((header[0]-5, header[1]+20, size[0]-10, 730))
        values.resize((values.width*3, values.height*3)).save(crop)
    text = private.run_checked(['tesseract', str(crop), 'stdout', '--psm', '6'], env=session.env)
    assert len(re.findall(r'(?<!\d)6(?!\d)', text)) == 2, text
    assert before == {str(path.relative_to(root)):hashlib.sha256(path.read_bytes()).hexdigest() for path in root.rglob('*.md')}
    assert not (Path(session.env['XDG_DATA_HOME'])/'little-screen-clip/search/index.sqlite').exists()
    (session.output/'review-checks.json').write_text(json.dumps({'judgment_varies_per_file':True,
        'literal_rule_reused':True, 'reasonable_first_value_keeps_per_file_review':True, 'implausible_header_does_not_override_body_answer':True,
        'source_content_unchanged':True, 'index_created':False}, indent=2))


def main():
    repo = Path(__file__).resolve().parent.parent
    args = application_arguments(__doc__, repo/'target/debug/clip-file-manager')
    output, root = media_fixture_directory(args)
    for name, summary in [('Alpha', 'This was excellent, delightful work.'),
                          ('Beta', 'The work was terrible and a disappointing failure.')]:
        (root/f'{name}.md').write_text(f'---\npositive: true\nsummary: {summary}\n---\nAn ordinary article.\n')
    (root/'Escalation').mkdir()
    (root/'Escalation/A-valid.md').write_text('---\nname: Alice\nage: 6\n---\nThe verified current age of the child Alice is 6 years.\n')
    (root/'Escalation/Alice.md').write_text('---\nname: Alice\nage: -7\n---\nThe header age is a corrupt import. The verified current age of the child Alice is 6 years.\n')
    return run_application_acceptance(args, output, root, exercise,
        'Real Q8 semantic review: differing interpretations for identical header layouts, literal reuse, contextual answers use body evidence despite corrupt header fields, no writes/index', environment)


if __name__ == '__main__':
    raise SystemExit(main())
