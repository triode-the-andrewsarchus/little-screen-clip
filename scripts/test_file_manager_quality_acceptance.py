"""Native timing observations distinguish partial, final and failed requests."""
from pathlib import Path
import tempfile
import sqlite3
import time
from types import SimpleNamespace
import unittest
from unittest.mock import patch
from file_manager_quality_acceptance import search_timings
from file_manager_real_corpus_acceptance import wait_index


class SearchTimingTests(unittest.TestCase):
    def test_seeded_ready_catalog_waits_for_completed_startup_generation(self):
        with tempfile.TemporaryDirectory() as temporary:
            output = Path(temporary)
            index = output / 'index.sqlite'
            with sqlite3.connect(index) as db:
                db.executescript('''CREATE TABLE roots(generation,state,error);
                    INSERT INTO roots VALUES(1,'ready',NULL);
                    CREATE TABLE memberships(verified); INSERT INTO memberships VALUES(1);
                    CREATE TABLE files(state,coverage,error); INSERT INTO files VALUES('ready','complete',NULL);
                    CREATE TABLE segments(modality,vector); INSERT INTO segments VALUES('text',X'00');''')
            session = SimpleNamespace(output=output, process=SimpleNamespace(poll=lambda: None))
            stages = []
            def reconcile(_):
                stages.append(len(stages))
                with sqlite3.connect(index) as db:
                    db.execute('UPDATE roots SET generation=2,state=?', ['building' if len(stages) == 1 else 'ready'])
                    db.execute('UPDATE memberships SET verified=?', [len(stages) > 1])
            with patch('file_manager_real_corpus_acceptance.time.sleep', side_effect=reconcile):
                result = wait_index(session, index, 20, time.monotonic(), after_generation=1)
            self.assertEqual(stages, [0, 1])
            self.assertEqual(result['generations'], [2])
            self.assertEqual(result['unverified_memberships'], 0)
            self.assertEqual(result['roots'], [('ready', None)])

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
