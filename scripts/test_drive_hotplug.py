"""Exercise protocol/ownership failure paths without a VM or host device access."""
import contextlib
import json
from pathlib import Path
import socket
import sys
import tempfile
import threading
import unittest
from unittest.mock import patch

from drive_tools.hotplug import DEVICE, UNPLUG, REPLUG, Qmp, UsbHotplug


class Peer:
    def __init__(self, root, replies):
        self.path = root / 'peer'
        self.socket = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.socket.bind(str(self.path))
        self.socket.listen(1)
        self.errors = []
        self.replies = replies
        self.thread = threading.Thread(target=self.serve)

    def __enter__(self):
        self.thread.start()
        return self

    def serve(self):
        try:
            self.socket.settimeout(5)
            connection, _ = self.socket.accept()
            with connection, connection.makefile('rb') as reader:
                connection.settimeout(5)
                connection.sendall(b'{"QMP":{"version":{},"capabilities":[]}}\n')
                for expected, messages in self.replies:
                    request = json.loads(reader.readline())
                    if request['execute'] != expected:
                        raise AssertionError(request)
                    for message in messages:
                        response = dict(message)
                        if 'event' not in response:
                            response.setdefault('id', request['id'])
                        connection.sendall(json.dumps(response).encode() + b'\n')
        except BaseException as error:
            self.errors.append(error)

    def __exit__(self, *_):
        self.thread.join(6)
        self.socket.close()
        if self.thread.is_alive() or self.errors:
            raise AssertionError(self.errors or 'QMP peer did not retire')


class Protocol(unittest.TestCase):
    def test_deleted_event_may_precede_reply_and_must_match_the_owned_device(self):
        for before in [False, True]:
            deletion = {'event': 'DEVICE_DELETED', 'data': {'device': DEVICE}}
            messages = [deletion, {'return': {}}] if before else [{'return': {}}, deletion]
            replies = [('qmp_capabilities', [{'return': {}}]),
                       ('device_del', [{'event': 'DEVICE_DELETED', 'data': {'device': 'foreign'}}, *messages])]
            with self.subTest(before=before), tempfile.TemporaryDirectory() as temp:
                with Peer(Path(temp), replies) as peer:
                    with contextlib.closing(Qmp(peer.path)) as client:
                        client.request('device_del', {'id': DEVICE})
                        client.deleted()
                        self.assertEqual(client.events, [{'event': 'DEVICE_DELETED', 'data': {'device': 'foreign'}}])

    def test_acknowledgement_without_deletion_event_is_not_completion(self):
        replies = [('qmp_capabilities', [{'return': {}}]), ('device_del', [{'return': {}}])]
        with tempfile.TemporaryDirectory() as temp, Peer(Path(temp), replies) as peer:
            with contextlib.closing(Qmp(peer.path)) as client:
                client.request('device_del', {'id': DEVICE})
                with self.assertRaises(ValueError):
                    client.deleted()

    def test_foreign_reply_and_command_error_are_refused(self):
        for response in [{'id': 'foreign', 'return': {}}, {'error': {'desc': 'denied'}}]:
            replies = [('qmp_capabilities', [{'return': {}}]), ('device_del', [response])]
            with self.subTest(response=response), tempfile.TemporaryDirectory() as temp:
                with Peer(Path(temp), replies) as peer:
                    with contextlib.closing(Qmp(peer.path)) as client:
                        with self.assertRaises(ValueError):
                            client.request('device_del', {'id': DEVICE})

    def test_a_second_deletion_cannot_reuse_the_first_completion_event(self):
        replies = [('qmp_capabilities', [{'return': {}}]),
                   ('device_del', [{'event': 'DEVICE_DELETED', 'data': {'device': DEVICE}}, {'return': {}}]),
                   ('device_del', [{'return': {}}])]
        with tempfile.TemporaryDirectory() as temp, Peer(Path(temp), replies) as peer:
            with contextlib.closing(Qmp(peer.path)) as client:
                client.request('device_del', {'id': DEVICE})
                client.deleted()
                client.request('device_del', {'id': DEVICE})
                with self.assertRaises(ValueError):
                    client.deleted()

    def test_an_old_device_event_cannot_confirm_a_new_generation(self):
        replies = [('qmp_capabilities', [{'return': {}}]),
                   ('device_del', [{'event': 'DEVICE_DELETED', 'data': {'device': DEVICE}}, {'return': {}}])]
        with tempfile.TemporaryDirectory() as temp, Peer(Path(temp), replies) as peer:
            with contextlib.closing(Qmp(peer.path)) as client:
                client.request('device_del', {'id': DEVICE + '-1'})
                with self.assertRaises(ValueError):
                    client.deleted(DEVICE + '-1')

    def test_repeated_state_keeps_the_failed_attempt_without_sending_another_command(self):
        with tempfile.TemporaryDirectory() as temp:
            replies = [('qmp_capabilities', [{'return': {}}]),
                       ('device_del', [{'return': {}}, {'event': 'DEVICE_DELETED', 'data': {'device': DEVICE}}]),
                       ('device_add', [{'return': {}}])]
            with Peer(Path(temp), replies) as peer, UsbHotplug(peer.path, Path(temp) / 'log') as usb:
                usb.action(UNPLUG)
                with self.assertRaises(ValueError):
                    usb.action(UNPLUG)
                usb.action(REPLUG)
                self.assertEqual([a['completed'] for a in usb.actions], [True, False, True])
                self.assertEqual([a['device'] for a in usb.actions], [DEVICE, DEVICE, DEVICE + '-1'])

    def test_controller_failure_retires_the_process_and_preserves_its_error(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            path = root / 'log'
            with UsbHotplug(root / 'missing', path) as usb, path.open('w') as log:
                command = [sys.executable, '-c', f'import time; print({UNPLUG!r}, flush=True); time.sleep(20)']
                with patch('drive_tools.hotplug.Qmp', side_effect=ValueError('controlled refusal')):
                    code, timed_out = usb.run(command, log, 5, ())
                self.assertLess(code, 0)
                self.assertFalse(timed_out)
                self.assertIn('controlled refusal', usb.error)
                self.assertEqual(usb.actions, [{'marker': UNPLUG, 'device': DEVICE, 'completed': False}])
                self.assertFalse(usb.report()['complete'])

    def test_timeout_retires_the_owned_process_without_claiming_a_control_action(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            path = root / 'log'
            with UsbHotplug(root / 'missing', path) as usb, path.open('w') as log:
                code, timed_out = usb.run([sys.executable, '-c', 'import time; time.sleep(20)'], log, 0.05, ())
                self.assertTrue(timed_out)
                self.assertLess(code, 0)
                self.assertEqual(usb.actions, [])


if __name__ == '__main__':
    unittest.main()
