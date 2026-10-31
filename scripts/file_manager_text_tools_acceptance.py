#!/usr/bin/env python3
"""Private large-document Q/A through isolated literal/regex text searches."""
import json
from pathlib import Path
import subprocess

from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance
from file_manager_answer_review_acceptance import completed
from file_manager_frontmatter_acceptance import environment
from file_manager_real_corpus_acceptance import observations
from file_manager_search_acceptance import submit
from search_model.setup import digest
from text_strip_acceptance import hover_observation


def isolated_search(binary):
    text = 'é中🦇 '*20000 + '\nRedemption code: WORKER-2049.\n'
    request = {'operation':'search', 'sources':[{'id':7,'text':text,'location':'page 19'}],
               'spec':{'queries':[{'mode':'regex','pattern':r'(?i)redemption\s+code'}], 'before':64,'after':128}}
    data = json.dumps(request).encode()
    assert len(data) > 128 * 1024
    process = subprocess.run([str(binary.resolve()),'--internal-file-extraction-plan'], input=data,
                             capture_output=True,check=True,timeout=5)
    result = json.loads(process.stdout)['Ok']
    assert result['kind'] == 'matches' and not result['limited'], result
    assert len(result['spans']) == 1, result
    span = result['spans'][0]
    assert span['source'] == 7
    exact = text.encode()[span['start']:span['end']].decode()
    assert 'WORKER-2049' in exact
    return {'request_bytes':len(data), 'exact_unicode_span':True, 'source':span['source']}


def exercise(session, root):
    before = observations(root)
    hashes = {path:digest(Path(path)) for path in before}
    session.observe('initial', ['Alpha.txt','Beta.txt'])
    session.click(647,160)
    index = Path(session.env['XDG_DATA_HOME'])/'little-screen-clip/search/index.sqlite'
    command = ':q:"What is the redemption code?";type:text'
    submit(session,command)
    log = completed(session,2)
    session.observe('late-codes',['ALPHA-2049','BETA-2031','Session answers'],timeout_seconds=90)
    assert log.count('column text search accepted') == 2, log
    x,y = session.word('late-codes','BETA-2031',minimum_y=280)
    hover_observation(session,'search-evidence',x+12,y,['text search UTF-8 bytes','Redemption code','BETA-2031'])
    submit(session, ':q:"What is the quantity?";type:number')
    table_log = completed(session,4)[len(log):]
    session.observe('table-quantities',['0 units','42 units','Session answers'],timeout_seconds=90)
    assert 'column text search accepted' not in table_log, 'An unscoped table quantity must retain full-document review'
    log = (session.output/'files.log').read_text()
    session.address(root/'Conflict')
    submit(session,command)
    conflict = completed(session,6)[len(log):]
    session.observe('competing-codes',['Ambiguous','Competing.txt','Repeated.txt'],timeout_seconds=90)
    assert 'column text search accepted' not in conflict, conflict
    critic_escalated = 'column text search escalated' in conflict
    session.address(root/'Records')
    submit(session, ':q:"What is the measurement in volts for record R0120?";type:number')
    completed(session, 7)
    session.observe('specific-record', ['72.50', 'Session answers'], timeout_seconds=90)
    assert observations(root) == before
    assert hashes == {path:digest(Path(path)) for path in before}
    assert not index.exists(), 'Q/A cannot enroll a root or create an index'
    (session.output/'text-search-checks.json').write_text(json.dumps({
        'direct_answers_from_late_search_windows':True, 'three_column_table_zero_and_number':True, 'unscoped_table_retains_full_document_review':True,
        'specific_repeated_record_answered':True, 'critic_escalation_exercised':critic_escalated, 'exact_offset_evidence_visible':True,
        'competing_labels_fall_back_without_confident_value':True,
        'source_metadata_and_bytes_unchanged':True,'index_created':False,
    },indent=2))


def main():
    repo = Path(__file__).resolve().parent.parent
    args = application_arguments(__doc__,repo/'target/debug/clip-file-manager')
    output,root = media_fixture_directory(args)
    (output/'isolated-search.json').write_text(json.dumps(isolated_search(args.binary),indent=2))
    background = ('These terms describe ordinary delivery, storage and handling procedures. '*500)+'\n'
    for name,code,quantity in [('Alpha','ALPHA-2049',0),('Beta','BETA-2031',42)]:
        (root/f'{name}.txt').write_text(background + f'Redemption code: {code}.\nThis code identifies the voucher.\n\nSKU | Quantity | Unit\nZX-2049 | {quantity} | units\n')
    (root/'Records').mkdir()
    (root/'Records/Measurements.txt').write_text('\n'.join(
        f'Record R{i:04}: measurement {"72.50" if i == 120 else "0.00"} volts.' for i in range(240)))
    (root/'Conflict').mkdir()
    (root/'Conflict/Competing.txt').write_text(background +
        'Redemption code: OLD-111.\nCoupon code: NEW-222.\nBoth labels refer to the same redemption code, but disagree; neither has priority.\n')
    (root/'Conflict/Repeated.txt').write_text(background +
        'Redemption code: OLD-111.\nRedemption code: NEW-222.\nNeither value has priority.\n')
    return run_application_acceptance(args,output,root,exercise,
        'Late generic Q/A via bounded isolated regex/literal searches, per-file sufficiency review, competing evidence fallback, exact Unicode offsets, no writes/index',environment)


if __name__ == '__main__':
    raise SystemExit(main())
