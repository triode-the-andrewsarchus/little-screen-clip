#!/usr/bin/env python3
"""Wrap owned WMAv2 tone packets in RIFF xWMA with a cumulative decoded-size table."""
from pathlib import Path
import json
import struct
import subprocess
import tempfile


def chunk(tag,data):
    return tag+struct.pack('<I',len(data))+data+(b'\0' if len(data)%2 else b'')


def make(root):
    for name,channels,rate,seconds in [('tones.xwma',2,44100,4),('mono.xwma',1,22050,2)]:
        with tempfile.TemporaryDirectory(prefix='owned-xwma-') as directory:
            wav=Path(directory)/'encoded.wav'
            signal='0.45*sin(2*PI*220*t)'+('|0.25*sin(2*PI*880*t)' if channels==2 else '')
            subprocess.run(['ffmpeg','-v','error','-f','lavfi','-i',f'aevalsrc={signal}:s={rate}:d={seconds}',
                            '-c:a','wmav2','-b:a','128k','-flags:a','+bitexact','-fflags','+bitexact','-f','wav',str(wav)],check=True,timeout=15)
            source=wav.read_bytes();at=12;chunks={}
            while at<len(source):
                tag=source[at:at+4];length=struct.unpack_from('<I',source,at+4)[0]
                chunks[tag]=source[at+8:at+8+length];at+=8+length+(length%2)
            # Retain WAVEFORMATEX codec setup, supported by the xWMA demuxer.
            fmt=chunks[b'fmt '];data=chunks[b'data'];align=struct.unpack_from('<H',fmt,12)[0]
            decoded=json.loads(subprocess.check_output(['ffprobe','-v','error','-show_frames','-show_entries','frame=nb_samples','-of','json',str(wav)],timeout=15))['frames']
            frames=[frame['nb_samples'] for frame in decoded]
            assert len(data)%align==0
            cumulative=[];count=0
            for index in range(len(data)//align):
                if index>0 and index-1<len(frames): count+=frames[index-1]
                cumulative.append(count*channels*2)
            body=b'XWMA'+chunk(b'fmt ',fmt)+chunk(b'dpds',struct.pack('<'+'I'*len(cumulative),*cumulative))+chunk(b'data',data)
            (root/name).write_bytes(chunk(b'RIFF',body))


if __name__=='__main__':
    make(Path(__file__).resolve().parents[1]/'crates/little-clip-core/tests/fixtures/audio')
