"""Stage generated companion bundles before replacing an existing build artifact."""
from contextlib import contextmanager
from pathlib import Path
import shutil
import tempfile


@contextmanager
def package_directory(output):
    output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='clip-helper-', dir=output.parent) as temporary:
        stage = Path(temporary) / 'helper'
        stage.mkdir()
        yield stage
        if output.exists():
            shutil.rmtree(output)
        stage.rename(output)
