#!/usr/bin/env python3
"""Controlled native CUDA compute failures recover on actual Vulkan helpers."""
import hashlib
import json
from pathlib import Path
import sqlite3
import subprocess

from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance
from file_manager_frontmatter_acceptance import environment
from file_manager_maintenance_acceptance import enroll_folder
from file_manager_quality_acceptance import measure_ui_queries
from file_manager_scale_acceptance import model_worker, vector_fingerprint
from file_manager_search_acceptance import submit


def recovered_backend(session, role, label):
    failed = int((session.output/f'failed-{role}.pid').read_text())
    assert not Path(f'/proc/{failed}').exists(), 'Failed helper was not reaped before recovery'
    worker = model_worker(session, role)
    assert '--device vulkan' in worker, worker
    session.click(993, 118)
    session.observe(label, ['Inference', 'Vulkan', 'fallback from cuda'])
    session.key('Escape')
    return {'failed_cuda_pid':failed, 'replacement':worker}


def exercise(session, root):
    indexed, questions = root/'Indexed', root/'Questions'
    originals = {str(path):hashlib.sha256(path.read_bytes()).hexdigest() for path in root.rglob('*.txt')}
    index, (_, paths, vectors) = enroll_folder(session, indexed)
    assert paths==[str(indexed/'Search.txt')] and vectors==1
    embedding = recovered_backend(session, 'embedding', 'embedding-recovered')
    fingerprint = vector_fingerprint(index)
    measure_ui_queries(session, ':emb:"Search.txt"', 1, report_name='recovered-embedding-query.json',
                       scope='One actual Vulkan query after controlled native CUDA failure')
    session.observe('embedding-query', ['Search.txt', 'Semantic candidates'])
    assert model_worker(session)==embedding['replacement'], 'Successful backend was not retained'
    with sqlite3.connect(index) as db:
        assert db.execute('SELECT count(*) FROM file_retries').fetchone()[0]==0, 'Backend recovery consumed a file retry'
    session.key('Escape')
    session.address(questions)
    submit(session, ':q:"What numeric score is explicitly stated?";type:number')
    session.observe('typed-answer', ['42', 'Record.txt', 'Session answers'], timeout_seconds=180)
    with sqlite3.connect(index) as db:
        assert db.execute('SELECT path FROM paths').fetchall()==[(bytes(indexed/'Search.txt'),)], 'Q/A implicitly indexed another folder'
    # Clicking a red Index button is enrollment, even when opening its popup.
    # Inspect the shared model status from the already enrolled location.
    session.address(indexed)
    answer = recovered_backend(session, 'answer', 'answer-recovered')
    assert vector_fingerprint(index)==fingerprint, 'Answer recovery changed stored embeddings'
    with sqlite3.connect(index) as db:
        assert db.execute('SELECT path FROM paths').fetchall()==[(bytes(indexed/'Search.txt'),)], 'Q/A implicitly indexed another folder'
    assert originals=={str(path):hashlib.sha256(path.read_bytes()).hexdigest() for path in root.rglob('*.txt')}
    session.required_closed_processes = [int(answer['replacement'].split()[0])]
    (session.output/'backend-recovery.json').write_text(json.dumps({
        'embedding':embedding, 'answer':answer, 'vector_fingerprint':fingerprint,
        'fault':'The private preload returns native llama_process status -2 once per CUDA role; no GPU memory exhaustion is induced',
        'scope':'Actual embedding and typed Q/A requests recover on Vulkan; failed helpers gone, visible fallback status, sticky backend, no consumed file retry/implicit enrollment/source changes; CPU exhaustion/cancellation/deadline/pipeline mismatch covered by components',
    }, indent=2))


def main():
    repo = Path(__file__).resolve().parent.parent
    args = application_arguments(__doc__, repo/'target/release/clip-file-manager')
    output, root = media_fixture_directory(args)
    for directory in ['Indexed','Questions']:
        (root/directory).mkdir()
    (root/'Indexed/Search.txt').write_text('An ordinary owned document about rivers and forests.\n')
    (root/'Questions/Record.txt').write_text('A validation record explicitly states a numeric score of 42 points.\n')
    library = output/'backend-failure.so'
    subprocess.run(['cc','-shared','-fPIC','-O2','-Wall','-Wextra','-Werror',str(repo/'scripts/fixtures/search_backend_failure.c'),'-ldl','-o',str(library)],check=True)
    def prepare(env, children):
        environment(env, children)
        env.update(LD_PRELOAD=str(library), CLIP_TEST_BACKEND_FAILURE_DIR=str(output),
                   CLIP_TEST_REAL_LLAMA=str(Path(env['XDG_DATA_HOME'])/'little-screen-clip/search/runtime/lib/libllama.so.0'))
    return run_application_acceptance(args, output, root, exercise,
        'Controlled native status -2 recovery from CUDA to Vulkan for embeddings and Q/A; private compositor only', prepare)


if __name__=='__main__':
    raise SystemExit(main())
