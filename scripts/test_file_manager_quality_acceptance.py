"""Native timing observations distinguish partial, final and failed requests."""
from pathlib import Path
import tempfile
import unittest
from file_manager_quality_acceptance import search_timings


class SearchTimingTests(unittest.TestCase):
    def test_request_tags_completion_and_errors_survive_log_parsing(self):
        with tempfile.TemporaryDirectory() as temporary:
            log = Path(temporary) / 'files.log'
            log.write_text('ordinary unrelated log\n'
                'DEBUG little_clip_search_timing: search results accepted request_id=7 complete=false succeeded=true elapsed_ms=12.5 entries=3\n'
                'DEBUG little_clip_search_timing: search results accepted request_id=7 complete=true succeeded=true elapsed_ms=25.75 entries=4\n'
                'DEBUG little_clip_search_timing: search results accepted request_id=8 complete=true succeeded=false elapsed_ms=3e1 entries=4\n')
            rows = search_timings(log)
            self.assertEqual([row['request_id'] for row in rows], [7, 7, 8])
            self.assertEqual([row['complete'] for row in rows], [False, True, True])
            self.assertEqual([row['succeeded'] for row in rows], [True, True, False])
            self.assertEqual([row['elapsed_ms'] for row in rows], [12.5, 25.75, 30.0])
            self.assertEqual([row['entries'] for row in rows], [3, 4, 4])

    def test_invalid_durations_cannot_supply_a_latency_measurement(self):
        with tempfile.TemporaryDirectory() as temporary:
            log = Path(temporary) / 'files.log'
            for duration in ('-1', 'nan', '1e309'):
                log.write_text('little_clip_search_timing: search results accepted request_id=1 complete=true succeeded=true '
                               f'elapsed_ms={duration} entries=0\n')
                with self.assertRaises(ValueError):
                    search_timings(log)


if __name__ == '__main__':
    unittest.main()
