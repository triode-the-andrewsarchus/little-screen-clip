"""Native timing diagnostics must reject missing, failed and invalid observations."""
import json
from pathlib import Path
import tempfile
import unittest
from answer_runtime_probe import attach_metrics, summarize


class AnswerTimingTests(unittest.TestCase):
    def write_observation(self, directory, metric, include_sampler=True, append=False):
        text = 'ordinary model log\nANSWER_PROBE '+json.dumps(metric)+'\n'
        if include_sampler:
            text += 'ANSWER_SAMPLER_PROBE {"init_cpu_ms":1,"sample_cpu_ms":5}\n'
        path = Path(directory)/'model.stderr.log'
        start = path.stat().st_size if append else 0
        with path.open('ab' if append else 'wb') as stream:
            stream.write(text.encode())
        return [start, path.stat().st_size]

    def test_native_rates_and_complete_request_latency_stay_distinct(self):
        with tempfile.TemporaryDirectory() as directory:
            bounds = self.write_observation(directory, {'status':0, 'prompt_tokens':1000, 'generated_tokens':80,
                                              'prompt_ms':200, 'generation_ms':1000})
            rows = [{'group':'warm-short', 'seconds':1.3, 'native_log_bytes':bounds}]
            attach_metrics(Path(directory), rows)
            self.assertEqual(summarize(rows), {'warm-short':{'samples':1, 'median_seconds':1.3,
                'median_prompt_tokens_per_second':5000, 'median_generated_tokens_per_second':80}})

    def test_request_intervals_group_applicability_calls_without_crossing_into_next_answer(self):
        with tempfile.TemporaryDirectory() as directory:
            metric = {'status':0, 'prompt_tokens':100, 'generated_tokens':10,
                      'prompt_ms':20, 'generation_ms':100}
            first = self.write_observation(directory, metric)
            second = self.write_observation(directory, metric, append=True)
            third = self.write_observation(directory, {**metric, 'generated_tokens':30}, append=True)
            rows = [{'native_log_bytes':[first[0], second[1]]}, {'native_log_bytes':third}]
            attach_metrics(Path(directory), rows)
            self.assertEqual([len(row['native_calls']) for row in rows], [2, 1])
            self.assertEqual(rows[0]['native']['generated_tokens'], 20)
            self.assertEqual(rows[0]['sampler']['sample_cpu_ms'], 10)
            self.assertEqual(rows[1]['generated_tokens_per_second'], 300)

    def test_missing_observations_cannot_supply_a_measurement(self):
        with tempfile.TemporaryDirectory() as directory:
            bounds = self.write_observation(directory, {}, include_sampler=False)
            with self.assertRaisesRegex(AssertionError, 'Missing native'):
                attach_metrics(Path(directory), [{'native_log_bytes':bounds}])

    def test_failed_generation_and_nonfinite_or_nonpositive_metrics_are_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            valid = {'status':0, 'prompt_tokens':1000, 'generated_tokens':80,
                     'prompt_ms':200, 'generation_ms':1000}
            invalid = [{'status':2}]
            invalid += [{field:value} for field in ('prompt_ms', 'generation_ms', 'prompt_tokens', 'generated_tokens')
                        for value in (0, -1, float('nan'), float('inf'))]
            for changed in invalid:
                bounds = self.write_observation(directory, {**valid, **changed})
                with self.assertRaises(AssertionError):
                    attach_metrics(Path(directory), [{'native_log_bytes':bounds}])


if __name__ == '__main__':
    unittest.main()
