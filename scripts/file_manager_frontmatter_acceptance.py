#!/usr/bin/env python3
"""Reusable metadata Q/A rules in the private Weston/Niri compositor."""
import hashlib
import json
import re
from pathlib import Path

from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance
from file_manager_answer_acceptance import model_environment
from file_manager_search_acceptance import submit


def exercise(session, root):
    before = {p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in root.glob('*.md')}
    session.observe('initial', ['Alpha.md', 'Beta.md', 'Gamma.md'])
    session.click(647, 160)
    index = Path(session.env['XDG_DATA_HOME']) / 'little-screen-clip/search/index.sqlite'
    submit(session, ':q:"What is the publication date?";type:date')
    session.observe('dates-ready', ['2021-11-18', '2022-03-04', '2023-06-07', 'Session answers'], timeout_seconds=60)
    text = (session.output / 'files.log').read_text()
    plans = re.findall(r'column metadata plan.*?cache_hit=(true|false)', text)
    answers = re.findall(r'column answer ready.*?elapsed_ms=([0-9.]+).*?structured=(true|false)', text)
    assert plans == ['false', 'true', 'false'], plans
    assert len(answers) == 3 and all(flag == 'true' for _, flag in answers), answers
    (session.output / 'extraction-timings.json').write_text(json.dumps({'plans_cache_hits':plans, 'answers_ms':[float(ms) for ms, _ in answers]}, indent=2))
    assert before == {p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in root.glob('*.md')}
    assert not index.exists(), 'Q/A must not implicitly index the folder'
    session.key('Escape')
    header = session.word('dates-ready', 'What', minimum_y=230)
    session.click(*header)
    session.observe('sorted-dates', ['2021-11-18', '2022-03-04', '2023-06-07'])
    positions = [session.word('sorted-dates', name, minimum_y=280)[1] for name in ['Alpha.md', 'Beta.md', 'Gamma.md']]
    assert positions == sorted(positions), positions
    session.address(root / 'Fallback')
    submit(session, ':q:"What is the publication date?";type:date')
    session.observe('generic-fallback', ['2018-09-10'], timeout_seconds=60)
    session.address(root)
    session.observe('return-keeps-dates', ['2021-11-18', '2022-03-04', '2023-06-07'])
    assert not index.exists()


def environment(env, children):
    model_environment(env, children)
    env['LITTLE_CLIP_SEARCH_TIMINGS'] = '1'


def main():
    repository = Path(__file__).resolve().parent.parent
    args = application_arguments(__doc__, repository / 'target/debug/clip-file-manager')
    output, root = media_fixture_directory(args)
    for name, header in [('Alpha', '---\ntitle: First\ndate: 2021-11-18\ndraft: false\n---'),
                         ('Beta', '---\ntitle: Second\ndate: 2022-03-04\ndraft: true\n---'),
                         ('Gamma', '+++\ntitle = "Third"\ndate = 2023-06-07\ndraft = false\n+++')]:
        (root / f'{name}.md').write_text(header + '\nBody mentions the unrelated date 2030-12-31.\n' + 'Ordinary prose. ' * 4000)
    (root / 'Fallback').mkdir()
    (root / 'Fallback/Plain.txt').write_text('This article was published on 2018-09-10. No other date is given.')
    return run_application_acceptance(args, output, root, exercise,
        'Real Q8 metadata planning, YAML/TOML execution, same-schema reuse, date sorting, generic fallback, no writes or implicit index', environment)


if __name__ == '__main__':
    raise SystemExit(main())
