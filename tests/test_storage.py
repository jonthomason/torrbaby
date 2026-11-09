import json
from pathlib import Path
import tempfile
import unittest
import uuid

from storage import Database


class StorageTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.path = Path(self.tmp.name) / 'state.sqlite3'
        self.db = Database(self.path)
        self.addCleanup(self.db.close)
        self.client = str(uuid.uuid4())
        self.db.register(self.client, 'Mac', 'macOS')

    def begin(self, client=None):
        return self.db.begin('a' * 40, 0, 'Movie.mp4', 123, client or self.client)['session_id']

    def test_migration_once_and_restart(self):
        legacy = Path(self.tmp.name) / 'legacy.json'
        state = {'jobs': {'a': {'phase': 'done'}}, 'settings': {'paused': True}, 'events': [], 'scans': ['pending']}
        legacy.write_text(json.dumps(state))
        target = Path(self.tmp.name) / 'migrated.sqlite3'
        db = Database(target, legacy)
        self.assertEqual(db.load_state(), state)
        state['scans'] = []
        db.save_state(state)
        db.close()
        db = Database(target, legacy)
        self.assertEqual(db.load_state(), state)
        self.assertEqual(json.loads(legacy.read_text())['scans'], ['pending'])
        db.close()

    def test_remote_rename_survives_registration(self):
        self.db.rename_client(self.client, 'Living room')
        self.assertEqual(self.db.register(self.client, 'Mac', 'macOS')['name'], 'Living room')
        with self.assertRaises(ValueError):
            self.db.register('not-uuid', 'Mac', 'macOS')

    def test_ordered_progress_resume_and_cross_device_conflict(self):
        first = self.begin()
        self.assertTrue(self.db.progress(first, 1, 120, 1000)['accepted'])
        self.assertFalse(self.db.progress(first, 0, 10, 1000)['accepted'])
        other = str(uuid.uuid4())
        self.db.register(other, 'iPad', 'iPadOS')
        next_ = self.db.begin('a' * 40, 0, 'Renamed.mp4', 123, other)
        self.assertEqual(next_['resume_position'], 120)
        self.assertFalse(self.db.progress(first, 2, 900, 1000)['accepted'])
        self.db.progress(next_['session_id'], 1, 300, 1000, True)
        self.assertFalse(self.db.progress(next_['session_id'], 2, 400, 1000)['accepted'])
        row = self.db.history()[0]
        self.assertEqual(row['position'], 300)
        self.assertEqual(row['name'], 'Renamed.mp4')
        self.assertEqual(row['client_name'], 'iPad')

    def test_watched_and_explicit_reset_override_reports(self):
        session = self.begin()
        self.db.progress(session, 0, 960, 1000)
        self.assertTrue(self.db.history()[0]['watched'])
        self.assertEqual(self.db.begin('a' * 40, 0, 'Movie.mp4', 123, self.client)['resume_position'], 0)
        session = self.begin()
        self.db.mark('a' * 40, 0, 'Movie.mp4', 123, False)
        self.assertFalse(self.db.progress(session, 100, 999, 1000)['accepted'])
        self.assertFalse(self.db.history()[0]['watched'])
        self.assertEqual(self.db.history()[0]['position'], 0)

    def test_invalid_progress_cannot_corrupt_history(self):
        session = self.begin()
        for position, duration in [(float('nan'), 100), (0, float('inf')), (-1, 100), (0, 0), (200, 100), (True, 100)]:
            with self.assertRaises(ValueError):
                self.db.progress(session, 0, position, duration)
        self.assertEqual(self.db.history()[0]['position'], 0)
