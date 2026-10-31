#!/usr/bin/env python3
"""Measure Q8 answer latency and native token rates without changing the installed runtime.

Uses production prompts/grammar/settings and synthetic documents. A private shim
copy enables llama.cpp counters and prints numeric metrics only to stderr.
"""
import argparse
import json
import math
from pathlib import Path
import statistics
import subprocess
import time
from types import SimpleNamespace

from answer_smoke import CASES, check
from quality import Worker
from runtime_probe import Sampler
from setup import digest


WRAPPER = r'''
#undef clip_answer_generate
int clip_answer_generate(clip_answer * answer, const char * system, const char * user,
                         const char * grammar, char * output, size_t size) {
    llama_perf_context_reset(answer->context);
    int status = clip_answer_generate_impl(answer, system, user, grammar, output, size);
    llama_synchronize(answer->context);
    struct llama_perf_context_data perf = llama_perf_context(answer->context);
    fprintf(stderr, "ANSWER_PROBE {\"prompt_tokens\":%d,\"generated_tokens\":%d,\"prompt_ms\":%.6f,\"generation_ms\":%.6f,\"status\":%d}\n",
        perf.n_p_eval, perf.n_eval, perf.t_p_eval_ms, perf.t_eval_ms, status);
    fflush(stderr);
    return status;
}
'''

def instrument(runtime, output):
    private = output/'runtime'
    library = private/'lib'
    library.mkdir(parents=True)
    for path in (runtime/'lib').iterdir():
        if path.name != 'libclip-answer.so':
            (library/path.name).symlink_to(path.resolve())
    (private/'runtime.json').symlink_to(runtime/'runtime.json')
    source = Path(__file__).with_name('answer_shim.c').read_text()
    assert digest(Path(__file__).with_name('answer_shim.c')) == json.loads((runtime/'runtime.json').read_text())['answer_shim_sha256'], 'Installed and checkout answer shims differ'
    marker = 'context.n_ctx = ANSWER_CONTEXT;'
    assert source.count(marker) == 1
    source = source.replace(marker, marker+'\n    context.no_perf = false;')
    marker = '    struct llama_sampler * constraint = llama_sampler_init_grammar'
    assert source.count(marker) == 1
    source = source.replace(marker, '    clock_t init_start = clock();\n'+marker)
    marker = '    llama_sampler_chain_add(sampler, llama_sampler_init_greedy());'
    assert source.count(marker) == 1
    source = source.replace(marker, marker+'\n    double init_cpu_ms = (clock()-init_start)*1000.0/CLOCKS_PER_SEC;\n    double sample_cpu_ms = 0.0;')
    marker = '        llama_token token = llama_sampler_sample(sampler, answer->context, -1);'
    assert source.count(marker) == 1
    source = source.replace(marker, '        clock_t sample_start = clock();\n'+marker)
    marker = '        if (llama_vocab_is_eog(answer->vocab, token))'
    assert source.count(marker) == 1
    source = source.replace(marker, '        sample_cpu_ms += (clock()-sample_start)*1000.0/CLOCKS_PER_SEC;\n'+marker)
    marker = '    llama_sampler_free(sampler);'
    assert source.count(marker) == 1
    source = source.replace(marker, r'''    fprintf(stderr, "ANSWER_SAMPLER_PROBE {\"init_cpu_ms\":%.6f,\"sample_cpu_ms\":%.6f}\n", init_cpu_ms, sample_cpu_ms);
'''+marker)
    staged = output/'answer_probe.c'
    staged.write_text('#include <stdio.h>\n#include <time.h>\n#include <math.h>\n#define clip_answer_generate clip_answer_generate_impl\n'+source+WRAPPER)
    command = ['cc', '-O2', '-fPIC', '-shared', '-std=c11', str(staged),
        '-I', str(runtime/'prefix/include'), '-L', str(runtime/'lib'), '-Wl,-rpath,$ORIGIN',
        '-lmtmd', '-lllama', '-lggml', '-lggml-base', '-lm', '-o', str(library/'libclip-answer.so')]
    result = subprocess.run(command, capture_output=True, text=True, timeout=60)
    (output/'compile.log').write_text(result.stdout+result.stderr)
    result.check_returncode()
    return private


def probe_cases(args):
    long_case = ('structured-long', 'What is the measurement in volts for record R0120?', 'number',
        '\n'.join(f'Record R{i:04}: measurement {"72.50" if i == 120 else "0.00"} volts.' for i in range(240)),
        True, 'value', '72.50')
    cases = [(case, 'correctness') for case in CASES if not args.case or case[0] in args.case]
    short = next(case for case in CASES if case[0] == 'money-exact')
    cases += [(short, 'warm-short')] * args.samples
    cases += [(long_case, 'warm-long')] * args.samples
    return cases


def attach_metrics(output, rows):
    data = (output/'model.stderr.log').read_bytes()
    for row in rows:
        start, end = row.pop('native_log_bytes')
        records = {'ANSWER_PROBE':[], 'ANSWER_SAMPLER_PROBE':[]}
        for line in data[start:end].decode().splitlines():
            prefix, _, payload = line.partition(' ')
            if prefix in records:
                records[prefix].append(json.loads(payload))
        calls, samplers = records['ANSWER_PROBE'], records['ANSWER_SAMPLER_PROBE']
        assert calls and len(calls) == len(samplers), 'Missing native timing observations'
        for metric in calls:
            assert metric['status'] == 0, metric
            for field in ('prompt_ms', 'generation_ms', 'prompt_tokens', 'generated_tokens'):
                assert math.isfinite(metric[field]) and metric[field] > 0, metric
        row['native_calls'] = calls
        row['sampler_calls'] = samplers
        row['native'] = {field:sum(call[field] for call in calls)
                        for field in ('prompt_ms', 'generation_ms', 'prompt_tokens', 'generated_tokens')}
        row['sampler'] = {field:sum(call[field] for call in samplers)
                         for field in ('init_cpu_ms', 'sample_cpu_ms')}
        metric = row['native']
        row['prompt_tokens_per_second'] = metric['prompt_tokens']*1000/metric['prompt_ms']
        row['generated_tokens_per_second'] = metric['generated_tokens']*1000/metric['generation_ms']


def summarize(rows):
    summary = {}
    for group in ('correctness', 'warm-short', 'warm-long'):
        selected = [row for row in rows if row['group'] == group]
        if not selected:
            continue
        summary[group] = {'samples':len(selected)}
        for field in ('seconds', 'prompt_tokens_per_second', 'generated_tokens_per_second'):
            summary[group]['median_'+field] = statistics.median(row[field] for row in selected)
    return summary


def evaluate(args, output, runtime):
    sampler = Sampler()
    worker = None
    rows = []
    started = time.monotonic()
    try:
        worker = Worker(SimpleNamespace(runtime=runtime, models=args.models, device='cuda'), output,
            extra_args=('--role', 'answer', '--answer-provider', 'qwen3.5-9b'), on_spawn=sampler.start)
        load_seconds = time.monotonic()-started
        assert worker.ready['backend'] == 'cuda', worker.ready
        cases = probe_cases(args)
        for case, group in cases:
            name, question, kind, text, complete, _, _ = case
            before = time.monotonic()
            log_start = (output/'model.stderr.log').stat().st_size
            try:
                response = worker.request({'op':'answer', 'question':question, 'type':kind,
                    'sources':[{'id':0, 'text':text}], 'complete':complete})
                result = check(case, response)
            except RuntimeError as error:
                # A rejected model response is a failed case, not a lost report.
                result = {'case':name, 'passed':False, 'error':str(error)}
            rows.append({'group':group, 'seconds':time.monotonic()-before,
                'native_log_bytes':[log_start, (output/'model.stderr.log').stat().st_size], **result})
            (output/'progress.json').write_text(json.dumps({'completed':len(rows), 'total':len(cases),
                'last_case':name, 'passed':sum(row['passed'] for row in rows)}))
        ready = worker.ready
    finally:
        if worker:
            worker.close()
        sampler.close()
    attach_metrics(output, rows)
    return {'ready':ready, 'load_seconds':load_seconds, 'elapsed_seconds':time.monotonic()-started,
        'passed':sum(row['passed'] for row in rows), 'total':len(rows), 'summary':summarize(rows), 'requests':rows,
        'resources':{'peaks':sampler.peaks, 'samples':list(sampler.records)},
        'scope':'Actual production answer prompts, grammar and settings; private shim differs only by native performance counters/wrapper; synthetic text only; warm model pages, not disk-cold; no p95 or general accuracy claim'}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--runtime', type=Path, required=True)
    parser.add_argument('--models', type=Path, required=True)
    parser.add_argument('--output-dir', type=Path, required=True)
    parser.add_argument('--samples', type=int, default=5)
    parser.add_argument('--case', action='append', choices=[case[0] for case in CASES], default=[])
    args = parser.parse_args()
    if not 1 <= args.samples <= 20:
        parser.error('Use 1..20 samples')
    args.runtime = args.runtime.resolve(strict=True)
    args.models = args.models.resolve(strict=True)
    output = args.output_dir.resolve()
    output.mkdir(parents=True, exist_ok=False)
    runtime = instrument(args.runtime, output)
    report = evaluate(args, output, runtime)
    report['sha256'] = {name:digest(Path(__file__).with_name(name)) for name in
        ('answer_shim.c', 'answers.py', 'worker.py', 'answer_runtime_probe.py')}
    report['model_sha256'] = digest(args.models/'Qwen_Qwen3.5-9B-Q8_0.gguf')
    report['private_shim_source_sha256'] = digest(output/'answer_probe.c')
    (output/'report.json').write_text(json.dumps(report, indent=2))
    print(json.dumps({key:report[key] for key in ('passed', 'total', 'load_seconds', 'summary')}))
    return int(report['passed'] != report['total'])


if __name__ == '__main__':
    raise SystemExit(main())
