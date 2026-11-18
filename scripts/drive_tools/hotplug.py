"""Control only the runner's emulated USB disk through its private QMP socket."""
import json
from pathlib import Path
import socket
import subprocess
import time

DEVICE = 'owned-usb-media'
BACKEND = 'owned-usb-backend'
BUS = 'owned-usb.0'
SERIAL = 'clip-owned-media'
UNPLUG = 'LITTLE_CLIP_USB_UNPLUG=1'
REPLUG = 'LITTLE_CLIP_USB_REPLUG=1'
LIMIT = 65536


class Qmp:
    """Bounded messages and matched replies; deletion also requires its event."""
    def __init__(self, path):
        self.socket = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.socket.settimeout(10)
        self.reader = None
        self.events = []
        self.sequence = 0
        try:
            self.socket.connect(str(path))
            self.reader = self.socket.makefile('rb')
            if 'QMP' not in self.receive():
                raise ValueError('QMP greeting missing')
            self.request('qmp_capabilities')
        except BaseException:
            self.close()
            raise

    def close(self):
        if self.reader is not None:
            self.reader.close()
        self.socket.close()

    def receive(self):
        line = self.reader.readline(LIMIT + 1)
        if not line or len(line) > LIMIT:
            raise ValueError('QMP connection closed or message exceeded its budget')
        message = json.loads(line)
        if not isinstance(message, dict):
            raise ValueError('QMP response must be an object')
        if 'event' in message:
            if len(self.events) >= 128:
                raise ValueError('QMP event budget exceeded')
            self.events.append(message)
        return message

    def request(self, command, arguments=None):
        self.sequence += 1
        identity = self.sequence
        request = {'execute': command, 'id': identity}
        if arguments is not None:
            request['arguments'] = arguments
        self.socket.sendall(json.dumps(request).encode() + b'\n')
        while True:
            response = self.receive()
            if 'event' in response:
                continue
            if response.get('id') != identity:
                raise ValueError('QMP reply does not match its command')
            if 'error' in response:
                raise ValueError(f'QMP {command} refused: {response["error"]}')
            if 'return' not in response:
                raise ValueError('QMP command has no completion result')
            return response['return']

    def deleted(self, device=DEVICE):
        while True:
            for index, event in enumerate(self.events):
                if event.get('event') == 'DEVICE_DELETED' and event.get('data', {}).get('device') == device:
                    del self.events[index]
                    return
            if 'event' not in self.receive():
                raise ValueError('Unexpected QMP reply while awaiting device deletion')


class UsbHotplug:
    """Read explicit guest markers, retaining every attempted control outcome."""
    def __init__(self, socket_path, log_path):
        self.socket_path = Path(socket_path)
        self.log_path = Path(log_path)
        self.reader = None
        self.client = None
        self.partial = ''
        self.plugged = True
        self.device = DEVICE
        self.generation = 0
        self.actions = []
        self.error = None

    def __enter__(self):
        return self

    def __exit__(self, *_):
        if self.reader is not None:
            self.reader.close()
        if self.client is not None:
            self.client.close()

    def arguments(self, disk):
        # The caller already holds the validated, owned regular-file descriptor.
        return ['-qmp', f'unix:{self.socket_path},server=on,wait=off',
                '-device', 'qemu-xhci,id=owned-usb',
                '-blockdev', json.dumps({'driver': 'file', 'node-name': 'owned-usb-file', 'filename': f'/proc/self/fd/{disk}'}),
                '-blockdev', json.dumps({'driver': 'raw', 'node-name': BACKEND, 'file': 'owned-usb-file'}),
                '-device', f'usb-storage,id={DEVICE},drive={BACKEND},bus={BUS},removable=on,serial={SERIAL}']

    def action(self, marker):
        record = {'marker': marker, 'device': self.device, 'completed': False}
        self.actions.append(record)
        if marker not in (UNPLUG, REPLUG):
            raise ValueError('Unrecognized USB marker')
        if (marker == UNPLUG) != self.plugged:
            raise ValueError('USB marker repeats an already completed state')
        if self.client is None:
            self.client = Qmp(self.socket_path)
        if marker == UNPLUG:
            self.client.request('device_del', {'id': self.device})
            self.client.deleted(self.device)
        elif marker == REPLUG:
            self.generation += 1
            record['device'] = f'{DEVICE}-{self.generation}'
            self.client.request('device_add', {'driver': 'usb-storage', 'id': record['device'],
                                              'drive': BACKEND, 'bus': BUS, 'removable': True,
                                              'serial': SERIAL})
            self.device = record['device']
        self.plugged = marker == REPLUG
        record['completed'] = True

    def poll(self):
        if self.reader is None:
            self.reader = self.log_path.open(errors='replace')
        chunk = self.reader.read(LIMIT)
        if not chunk:
            return False
        self.partial += chunk
        while '\n' in self.partial:
            line, self.partial = self.partial.split('\n', 1)
            if len(line) > LIMIT:
                raise ValueError('Guest serial line exceeded its budget')
            marker = line.strip()
            if marker in (UNPLUG, REPLUG):
                self.action(marker)
        if len(self.partial) > LIMIT:
            raise ValueError('Guest serial line exceeded its budget')
        return True

    def report(self):
        return dict(enabled=True, complete=self.error is None and all(a['completed'] for a in self.actions),
                    actions=self.actions, error=self.error)

    def run(self, command, log, timeout, pass_fds):
        deadline = time.monotonic() + timeout
        timed_out = False
        with subprocess.Popen(command, stdout=log, stderr=subprocess.STDOUT, pass_fds=pass_fds) as process:
            try:
                while True:
                    data = self.poll()
                    code = process.poll()
                    if code is not None and not data:
                        return code, timed_out
                    if code is None and time.monotonic() >= deadline:
                        timed_out = True
                        process.kill()
                        return process.wait(), timed_out
                    time.sleep(0.05)
            except BaseException as error:
                self.error = str(error)
                if process.poll() is None:
                    process.kill()
                return process.wait(), timed_out
