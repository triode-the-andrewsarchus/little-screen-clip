"""Native timing diagnostics must reject missing, failed and invalid observations."""
import json
from pathlib import Path
import tempfile
import unittest
from answer_runtime_probe import attach_metrics, summarize


class AnswerTimingTests(unittest.TestCase):
    def write_observation(self, directory, metric, include_sampler=True):
        text = 'ordinary model log\nANSWER_PROBE '+json.dumps(metric)+'\n'
        if include_sampler:
            text += 'ANSWER_SAMPLER_PROBE {"init_cpu_ms":1,"sample_cpu_ms":5}\n'
        (Path(directory)/'model.stderr.log').write_text(text)

    def test_native_rates_and_complete_request_latency_stay_distinct(self):
        with tempfile.TemporaryDirectory() as directory:
            self.write_observation(directory, {'status':0, 'prompt_tokens':1000, 'generated_tokens':80,
                                              'prompt_ms':200, 'generation_ms':1000})
            rows = [{'group':'warm-short', 'seconds':1.3}]
            attach_metrics(Path(directory), rows)
            self.assertEqual(summarize(rows), {'warm-short':{'samples':1, 'median_seconds':1.3,
                'median_prompt_tokens_per_second':5000, 'median_generated_tokens_per_second':80}})

    def test_missing_observations_cannot_supply_a_measurement(self):
        with tempfile.TemporaryDirectory() as directory:
            self.write_observation(directory, {}, include_sampler=False)
            with self.assertRaisesRegex(AssertionError, 'Missing native'):
                attach_metrics(Path(directory), [{}])

    def test_failed_generation_and_nonfinite_or_nonpositive_metrics_are_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            valid = {'status':0, 'prompt_tokens':1000, 'generated_tokens':80,
                     'prompt_ms':200, 'generation_ms':1000}
            invalid = [{'status':2}]
            invalid += [{field:value} for field in ('prompt_ms', 'generation_ms', 'prompt_tokens', 'generated_tokens')
                        for value in (0, -1, float('nan'), float('inf'))]
            for changed in invalid:
                self.write_observation(directory, {**valid, **changed})
                with self.assertRaises(AssertionError):
                    attach_metrics(Path(directory), [{}])


if __name__ == '__main__':
    unittest.main()
