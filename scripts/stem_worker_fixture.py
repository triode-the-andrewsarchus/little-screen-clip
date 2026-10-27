#!/usr/bin/env python3
"""Acceptance-only tensor model: real WAV I/O, no model loading or downloads."""
import json
import os
from pathlib import Path
import runpy
import sys


def main():
    import demucs.api
    import torch

    class FixtureSeparator:
        samplerate = 44100
        audio_channels = 2

        def __init__(self, model, progress):
            assert model == 'htdemucs' and progress

        def separate_tensor(self, tensor, sample_rate):
            assert sample_rate == self.samplerate and tensor.ndim == 2
            assert tensor.shape[0] == self.audio_channels and tensor.dtype == torch.float32
            report = os.environ.get('CLIP_STEM_FIXTURE_REPORT')
            if report:
                with open(report, 'a') as output:
                    output.write(json.dumps({'frames':tensor.shape[1], 'channels':tensor.shape[0]}) + '\n')
            return tensor, {name:tensor * factor for name, factor in
                            [('vocals', .4), ('drums', .3), ('bass', .2), ('other', .1)]}

    demucs.api.Separator = FixtureSeparator
    # A private runner launcher may stand in for the managed venv executable.
    # Report its name to the helper so that this harness is not re-executed.
    sys.executable = sys.argv[1]
    script = Path(sys.argv[2])
    sys.argv = [str(script), *sys.argv[3:]]
    runpy.run_path(str(script), run_name='__main__')


if __name__ == '__main__':
    main()
