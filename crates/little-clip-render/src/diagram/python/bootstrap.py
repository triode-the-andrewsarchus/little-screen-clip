"""Fixed importer, in-memory font metrics and resource policy for diagram engines."""
import base64
import bisect
import contextlib
import functools
import importlib
import importlib.abc
import importlib.util
import io
import itertools
import json
import re
import sys
import types
import xml.etree.ElementTree as ET


class EmbeddedModules(importlib.abc.MetaPathFinder, importlib.abc.Loader):
    def __init__(self, entries):
        self.entries = {name: (package, code) for name, package, code in entries}
        self.roots = {name.split('.')[0] for name in self.entries}

    def find_spec(self, fullname, path=None, target=None):
        if fullname in self.entries:
            return importlib.util.spec_from_loader(
                fullname, self, is_package=self.entries[fullname][0])
        if fullname.split('.')[0] in self.roots:
            raise ImportError('Unbundled diagram module is disabled: ' + fullname)
        return None

    def create_module(self, spec):
        return None

    def exec_module(self, module):
        # Entries come exclusively from fixed trusted include_str! tables.
        # Authored diagram text is never passed to Python's compiler/importer.
        trusted_source = self.entries[module.__name__][1]
        exec(compile(trusted_source, '<bundled ' + module.__name__ + '>', 'exec'),
             module.__dict__)


def denied(*args, **kwargs):
    raise ValueError('External diagram resources and plugins are disabled')


def resource_audit(event, args):
    if (event in {'open', 'import'}
            or event.startswith(('os.', 'socket.', 'urllib.', 'subprocess.', 'ctypes.'))):
        raise ValueError('External diagram resource access is disabled: ' + event)


class Fonts:
    def __init__(self, encoded):
        from PIL import ImageFont
        self.ImageFont = ImageFont
        self.data = [base64.b64decode(value, validate=True) for value in encoded]
        self.face('sans-serif', 11, 'normal')

    @functools.lru_cache(maxsize=128)
    def face(self, family, size, style):
        italic = 'italic' in style or 'oblique' in style
        bold = 'bold' in style
        if family in ('serif', 'Times'):
            index = (7 if italic else 6) if bold else (8 if italic else 9)
        elif family in ('monospace', 'Courier'):
            index = (3 if italic else 2) if bold else (4 if italic else 5)
        else:
            index = 0 if italic else 1
        face = self.ImageFont.truetype(io.BytesIO(self.data[index]), size=max(1, min(256, round(size))))
        if index in (0, 1) and bold:
            face.set_variation_by_axes([700])
        return face


def main():
    name = 'Local diagram'
    try:
        request = json.load(sys.stdin)
        name = request['name']
        sys.meta_path.insert(0, EmbeddedModules(request['modules']))
        try:
            fonts = Fonts(request['fonts'])
        except ImportError as error:
            raise ValueError('Local diagram previews require Python 3 with Pillow') from error
        with contextlib.redirect_stdout(sys.stderr):
            render = prepare(fonts)
            # All trusted imports/setup finish before source enters any parser.
            sys.addaudithook(resource_audit)
            result = render(request['input'])
        if not isinstance(result, str) or len(result.encode('utf-8')) > 2 * 1024 * 1024:
            raise ValueError('Generated output exceeds the diagram preview budget')
        sys.stdout.write(result)
    except Exception as error:
        sys.stderr.write(name + ': ' + str(error)[:2048] + '\n')
        sys.exit(1)
