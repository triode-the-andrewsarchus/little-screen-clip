#!/usr/bin/env python3
"""Private real-model checks for literal fields mixed with prose and tables."""
import json
from pathlib import Path
import re

from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance
from file_manager_answer_review_acceptance import completed
from file_manager_frontmatter_acceptance import environment
from file_manager_real_corpus_acceptance import observations
from search_model.setup import digest
from file_manager_search_acceptance import submit
from text_strip_acceptance import hover_observation


COMMAND = ':q:"What is the stored recorded date field?";type:date'


def exercise(session, root):
    before = observations(root)
    hashes = {path:digest(Path(path)) for path in before}
    session.observe('initial', ['Alpha.txt', 'Beta.txt'])
    session.click(647, 160)
    index = Path(session.env['XDG_DATA_HOME'])/'little-screen-clip/search/index.sqlite'
    previous = ''
    cases = [
        ('prose', root, ['false','true']),
        ('markdown-table', root/'Tables', ['false','true']),
        ('tab-separated', root/'Tabs', ['false','true']),
        ('changed-prose', root/'Changed', ['false','false']),
    ]
    reports = []
    for number, (name, folder, expected_hits) in enumerate(cases, 1):
        if folder != root:
            session.address(folder)
        submit(session, COMMAND)
        log = completed(session, number * 2)
        delta = log[len(previous):]
        session.observe(name, ['2020-02-03', '2023-04-05', 'Session answers'], timeout_seconds=90)
        hits = re.findall(r'column metadata plan.*?cache_hit=(true|false)', delta)
        assert hits == expected_hits, (name, hits, delta)
        assert delta.count('column labelled field accepted') == 2, (name, delta)
        assert delta.count('structured=true') == 2, (name, delta)
        assert re.search(r'column candidate review.*action="?reuse', delta), (name, delta)
        reports.append({'case':name, 'cache_hits':hits, 'accepted_literal_fields':2})
        previous = log
    x, y = session.word('changed-prose', '2023-04-05', minimum_y=280)
    hover_observation(session, 'full-document-evidence', x + 12, y,
                      ['Recorded date', '2023-04-05', 'Verification note'])
    session.address(root/'Duplicate')
    submit(session, COMMAND)
    log = completed(session, 9)[len(previous):]
    session.observe('ambiguous-duplicate', ['Ambiguous', 'Repeated.txt'], timeout_seconds=90)
    assert 'column labelled field accepted' not in log, log
    assert observations(root) == before, 'Source metadata changed'
    assert hashes == {path:digest(Path(path)) for path in before}, 'Source bytes changed'
    assert not index.exists(), 'Q/A must not create an index or enroll roots'
    (session.output/'document-field-checks.json').write_text(json.dumps({
        'cases':reports, 'changed_prose_replanned':True, 'full_document_evidence_visible':True,
        'duplicate_uses_generic_ambiguity':True, 'sources_unchanged':True, 'index_created':False,
    },indent=2))


def main():
    repo = Path(__file__).resolve().parent.parent
    args = application_arguments(__doc__,repo/'target/debug/clip-file-manager')
    output, root = media_fixture_directory(args)
    for folder in ['Tables', 'Tabs', 'Changed', 'Duplicate']:
        (root/folder).mkdir()
    for name, date in [('Alpha','2020-02-03'), ('Beta','2023-04-05')]:
        prose = f'Survey report\nThe following observations were recorded.\nRecorded date = {date}\nSample count = 0\nEnd of report.\n'
        (root/f'{name}.txt').write_text(prose)
        (root/f'Tables/{name}.txt').write_text(
            f'Survey report\n| Field | Value |\n| --- | --- |\n| Recorded date | {date} |\n| Other date | 1999-01-01 |\nEnd of report.\n')
        (root/f'Tabs/{name}.txt').write_text(
            f'Survey report\nRecorded date\t{date}\nOther date\t1999-01-01\nEnd of report.\n')
        note = 'Verification note: The literal date field was independently checked.' if name == 'Beta' else 'The data was verified.'
        (root/f'Changed/{name}.txt').write_text(prose + note + '\n')
    (root/'Duplicate/Repeated.txt').write_text(
        'Survey report\nRecorded date: 2020-02-03\nRecorded date: 2023-04-05\nNeither value has priority.\n')
    return run_application_acceptance(args,output,root,exercise,
        'Literal fields in prose/Markdown/TSV, reviewed template reuse, changed prose forces review, duplicate ambiguity, source evidence, no writes/index', environment)


if __name__ == '__main__':
    raise SystemExit(main())
