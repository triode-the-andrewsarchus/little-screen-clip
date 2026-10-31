#!/usr/bin/env python3
"""Real five-minute model release/reload on the private Weston/Niri compositor."""
import json
from pathlib import Path
import tempfile
import time

from file_manager_acceptance import application_arguments, run_application_acceptance
from file_manager_frontmatter_acceptance import environment
from file_manager_maintenance_acceptance import enroll_folder
from file_manager_quality_acceptance import measure_ui_queries
from file_manager_scale_acceptance import model_worker, vector_fingerprint
import ui_acceptance_capture as private


def exercise(session, root):
    source = root/'Idle.txt'
    original = source.read_bytes()
    session.observe('initial', ['Idle.txt'])
    index, (_, paths, vectors) = enroll_folder(session, root)
    assert paths == [str(source)] and vectors == 1
    command = ':emb:"Idle.txt"'
    measure_ui_queries(session, command, 1, report_name='before-idle-query.json',
                       scope='One owned real embedding/source/UI request before idle release')
    session.observe('before-idle', ['Idle.txt', 'Semantic candidates'])
    fingerprint = vector_fingerprint(index)
    worker = model_worker(session)
    assert '--device cuda' in worker, worker
    pid = int(worker.split()[0])
    process = Path(f'/proc/{pid}')
    gpu = private.run_checked(['nvidia-smi', '--query-compute-apps=pid,used_gpu_memory',
                               '--format=csv,noheader,nounits'], env=session.env)
    allocation = [line.strip() for line in gpu.splitlines() if line.split(',')[0].strip() == str(pid)]
    assert len(allocation) == 1, 'Owned CUDA helper has no observed GPU allocation'
    started = time.monotonic()
    observations = []
    while process.exists():
        elapsed = time.monotonic() - started
        assert session.process.poll() is None, 'Files exited during idle observation'
        assert elapsed < 330, 'Idle helper remained alive beyond five minutes plus observation margin'
        observations.append({'elapsed_seconds':elapsed, 'helper_alive':True})
        (session.output/'idle-progress.json').write_text(json.dumps(observations, indent=2))
        time.sleep(1)
    elapsed = time.monotonic() - started
    # Capture/OCR before this timer may consume a few seconds of the idle period.
    assert elapsed >= 280, f'Unexpected early helper exit after {elapsed:.2f} seconds'
    assert source.read_bytes() == original
    assert vector_fingerprint(index) == fingerprint, 'Idle release changed canonical vectors'
    measure_ui_queries(session, command, 1, report_name='after-idle-query.json',
                       scope='One owned real embedding/source/UI request after idle reload')
    worker = model_worker(session)
    assert '--device cuda' in worker, worker
    successor = int(worker.split()[0])
    assert successor != pid and not process.exists(), 'Query did not replace the retired helper'
    session.required_closed_processes = [successor]
    session.observe('after-idle-reload', ['Idle.txt', 'Semantic candidates'])
    assert source.read_bytes() == original
    assert vector_fingerprint(index) == fingerprint
    (session.output/'idle-lifecycle.json').write_text(json.dumps({
        'released_pid':pid, 'successor_pid':successor, 'observed_idle_seconds':elapsed,
        'initial_owned_gpu_allocation':allocation[0], 'vector_fingerprint':fingerprint,
        'scope':'Real default idle timeout, CUDA helper exit, successful query reload and close cleanup; one text file, embedding role; not keep-loaded/OOM/answer-role native acceptance',
    }, indent=2))


def main():
    repo = Path(__file__).resolve().parent.parent
    args = application_arguments(__doc__, repo/'target/release/clip-file-manager')
    output = args.output_dir.resolve()
    output.mkdir(mode=0o700, parents=True)
    with tempfile.TemporaryDirectory(prefix='clip-search-idle-', dir=Path.home()/'Documents/type_testing') as folder:
        root = Path(folder)
        (root/'Idle.txt').write_text('An ordinary owned document about cats sleeping quietly.\n')
        return run_application_acceptance(args, output, root, exercise,
            'Actual five-minute CUDA embedding idle release and reload; private compositor only', environment)


if __name__ == '__main__':
    raise SystemExit(main())
