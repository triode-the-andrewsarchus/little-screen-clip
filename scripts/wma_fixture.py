#!/usr/bin/env python3
"""Create owned Windows Media tone recordings with explicit metadata."""
from pathlib import Path
import subprocess


def make(root):
    cases=[('tones.wma','wmav2','aevalsrc=0.45*sin(2*PI*220*t)|0.25*sin(2*PI*880*t):s=44100:d=4','Harbor tones'),
           ('mono.wma','wmav1','aevalsrc=0.35*sin(2*PI*330*t):s=22050:d=2','Harbor mono')]
    for name,codec,source,title in cases:
        subprocess.run(['ffmpeg','-v','error','-y','-f','lavfi','-i',source,'-fflags','+bitexact','-flags:a','+bitexact',
                        '-c:a',codec,'-b:a','128k','-metadata','title='+title,'-metadata','artist=Owned fixture',
                        '-metadata','album=Harbor collection','-metadata','creation_time=2026-11-09T00:00:00Z',str(root/name)],check=True,timeout=15)


if __name__=='__main__':
    make(Path(__file__).resolve().parents[1]/'crates/little-clip-core/tests/fixtures/audio')
