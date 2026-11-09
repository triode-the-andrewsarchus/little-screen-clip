#!/usr/bin/env python3
"""Generate owned sine-wave samples, SFZ, SoundFont and legacy NKI mapping fixtures."""
from pathlib import Path
import math
import struct
import wave
import zlib
from zipfile import ZipFile, ZIP_STORED, ZIP_DEFLATED


def chunk(tag,data):
    return tag+struct.pack('<I',len(data))+data+(b'\0' if len(data)%2 else b'')


def generators(zones):
    bags=[];values=[]
    for zone in zones:
        bags.append(struct.pack('<HH',len(values),0))
        values.extend(struct.pack('<HH',key,value&65535) for key,value in zone)
    bags.append(struct.pack('<HH',len(values),0));values.append(b'\0'*4)
    return b''.join(bags),b''.join(values)


def generate(root):
    root.mkdir(parents=True,exist_ok=True)
    rate=22050;frames=rate*2
    tones=[]
    for name,frequency,amplitude in [('soft',261.6256,0.25),('bright',329.6276,0.65)]:
        data=b''.join(struct.pack('<h',round(math.sin(i*math.tau*frequency/rate)*32767*amplitude)) for i in range(frames))
        tones.append(data)
        with wave.open(str(root/f'{name}.wav'),'wb') as wav:
            wav.setparams((1,2,rate,frames,'NONE','not compressed'));wav.writeframes(data)
    (root/'harbor.sfz').write_text('// Owned mappings, including a guarded textual include.\n#define $ROOT 60\n<control> default_path=.\n<global> pitch_keycenter=$ROOT\n#include "regions.sfzi"\n')
    (root/'regions.sfzi').write_text('<group> lokey=48 hikey=71\n<region> sample=soft.wav lovel=0 hivel=63 loop_start=10 loop_end=22049 region_label=Soft keys\n<region> sample=bright.wav lovel=64 hivel=127 region_label=Bright keys\n<group> lokey=36 hikey=47 pitch_keycenter=48\n<region> sample=soft.wav region_label=Bass keys\n')
    pbag,pgen=generators([[(43,36|(84<<8))],[(41,0)],[(41,1)]])
    ibag,igen=generators([[(51,0)],[(43,48|(71<<8)),(44,0|(63<<8)),(53,0)],[(43,48|(71<<8)),(44,64|(127<<8)),(53,1)],[(43,36|(47<<8)),(58,48),(53,0)]])
    phdr=b''.join(struct.pack('<20sHHHIII',name.encode(),program,0,bag,0,0,0) for name,program,bag in [('Harbor Keys',0,0),('Harbor Bass',1,2),('EOP',0,3)])
    inst=b''.join(struct.pack('<20sH',name.encode(),bag) for name,bag in [('Keys',0),('Bass',3),('EOI',4)])
    smpl=b''.join(tones)+b'\0'*92
    shdr=b''.join(struct.pack('<20sIIIIIBbHH',name.encode(),index*frames,(index+1)*frames,index*frames+10,index*frames+rate,rate,60,0,0,1) for index,name in enumerate(['Soft tone','Bright tone']))
    shdr+=struct.pack('<20sIIIIIBbHH',b'EOS',frames*2,frames*2,frames*2,frames*2,rate,0,0,0,1)
    pdta=b''.join(chunk(tag,data) for tag,data in [(b'phdr',phdr),(b'pbag',pbag),(b'pmod',b'\0'*10),(b'pgen',pgen),(b'inst',inst),(b'ibag',ibag),(b'imod',b'\0'*10),(b'igen',igen),(b'shdr',shdr)])
    body=b'sfbk'+chunk(b'LIST',b'INFO'+chunk(b'ifil',struct.pack('<HH',2,1))+chunk(b'INAM',b'Owned Harbor\0'))+chunk(b'LIST',b'sdta'+chunk(b'smpl',smpl))+chunk(b'LIST',b'pdta'+pdta)
    (root/'harbor.sf2').write_bytes(chunk(b'RIFF',body))
    zones=[]
    for name,sample,low,high,vl,vh in [('Soft keys','soft.wav',48,71,0,63),('Bright keys','bright.wav',48,71,64,127)]:
        params={'lowKey':low,'highKey':high,'lowVelocity':vl,'highVelocity':vh,'rootKey':60,'sampleStart':0,'sampleEnd':frames}
        values=''.join(f'<V name="{key}" value="{value}"/>' for key,value in params.items())
        zones.append(f'<NiSS_Zone name="{name}"><Parameters>{values}</Parameters><Sample><V name="file" value="{sample}"/></Sample></NiSS_Zone>')
    xml=f'<NiSS_Program name="Harbor legacy"><Groups><NiSS_Group name="Keys"><Zones>{"".join(zones)}</Zones></NiSS_Group></Groups></NiSS_Program>'.encode()
    header=struct.pack('>IIHHIIIIII',0xB36EE55E,36,0x50,2,0,0,1,0,frames*4,0)
    (root/'legacy.nki').write_bytes(header+zlib.compress(xml))
    (root/'native-compressed.nki').write_bytes(header+zlib.compress(xml.replace(b'Harbor legacy',b'Harbor compressed').replace(b'.wav',b'.ncw')))
    (root/'compressed.sfz').write_text('<region> sample=soft.ncw lokey=48 hikey=71 lovel=0 hivel=63 pitch_keycenter=60\n<region> sample=bright.ncw lokey=48 hikey=71 lovel=64 hivel=127 pitch_keycenter=60\n')
    mapping = f'''<DecentSampler name="Harbor Decent"><groups loNote="48" hiNote="71" rootNote="60">
<group name="Soft" loVel="0" hiVel="63"><sample path="soft.wav" end="{frames-1}" loopStart="10" loopEnd="22049"/></group>
<group name="Bright" loVel="64" hiVel="127"><sample path="bright.wav" end="{frames-1}"/></group>
<group name="Bass" loNote="36" hiNote="47" rootNote="48"><sample path="soft.wav" end="{frames-1}"/></group>
</groups></DecentSampler>'''
    (root/'harbor.dspreset').write_text(mapping+'\n')
    packed = f'''<multisample name="Harbor packed"><generator>Owned fixture</generator><group name="Keys"/>
<sample file="Samples/soft.wav" sample-stop="{frames}"><key root="60" low="48" high="71"/><velocity low="0" high="63"/></sample>
<sample file="Samples/bright.wav" sample-stop="{frames}"><key root="60" low="48" high="71"/><velocity low="64" high="127"/></sample>
<sample file="Samples/soft.wav" sample-stop="{frames}"><key root="48" low="36" high="47"/><velocity low="0" high="127"/></sample>
</multisample>'''
    with ZipFile(root/'harbor.multisample','w',compression=ZIP_STORED) as archive:
        archive.writestr('multisample.xml',packed)
        for sample in ['soft.wav','bright.wav']:
            archive.writestr('Samples/'+sample,(root/sample).read_bytes())
    with ZipFile(root/'harbor.dslibrary','w',compression=ZIP_DEFLATED) as archive:
        library = mapping.replace('Harbor Decent','Harbor library').replace('path="','path="../Samples/')
        archive.writestr('Presets/Harbor.dspreset',library)
        for sample in ['soft.wav','bright.wav']:
            archive.writestr('Samples/'+sample,(root/sample).read_bytes())
    generate_exs(root,rate,frames)
    generate_renoise(root,frames)


def generate_renoise(root,frames):
    samples=[]
    for name,low,high,alias in [('Soft keys',0,63,False),('Bright keys',64,127,False),('Bright slice',64,127,True)]:
        samples.append(f'<Sample><Name>{name}</Name><FileName>//File:/not-followed.wav</FileName><IsAlias>{str(alias).lower()}</IsAlias><LoopMode>Off</LoopMode><LoopStart>0</LoopStart><LoopEnd>{frames if not alias else 11025}</LoopEnd><SliceMarkers><SliceMarker><SamplePosition>11025</SamplePosition></SliceMarker></SliceMarkers><Mapping><Layer>Note-On Layer</Layer><BaseNote>60</BaseNote><NoteStart>{72 if alias else 48}</NoteStart><NoteEnd>{72 if alias else 71}</NoteEnd><VelocityStart>{low}</VelocityStart><VelocityEnd>{high}</VelocityEnd></Mapping></Sample>')
    with ZipFile(root/'harbor.xrni','w',compression=ZIP_DEFLATED) as archive:
        archive.writestr('Instrument.xml','<RenoiseInstrument doc_version="34"><Name>Harbor Renoise</Name><SampleGenerator><Samples>'+''.join(samples)+'</Samples></SampleGenerator></RenoiseInstrument>')
        for index,name in enumerate(['soft','bright']):
            archive.writestr(f'SampleData/Sample{index:02d} ({name}).wav',(root/f'{name}.wav').read_bytes())


def exs_record(big, kind, index, name, data):
    endian = ">" if big else "<"
    prefix = bytes([0 if big else 1, 1, 0, kind | (0x80 if big else 0x40)])
    length = len(data) | (0x8000 if big else 0)
    return (prefix + struct.pack(endian+'III',length,index,0)
            + (b'SOBT' if big else b'TBOS')
            + name.encode().ljust(64,b'\0') + data)

def generate_exs(root,rate,frames):
    for big in [False, True]:
        endian = '>' if big else '<'
        records = [exs_record(big,0,0,'Harbor EXS',struct.pack(endian+'IIII',0,2,1,2))]
        for index,(name,vl,vh) in enumerate([('Soft keys',0,63),('Bright keys',64,127)]):
            zone = bytearray(96)
            zone[0]=8;zone[1]=60;zone[6]=48;zone[7]=71;zone[9]=vl;zone[10]=vh
            struct.pack_into(endian+'IIII',zone,12,0,frames,10,22050)
            zone[33]=1
            struct.pack_into(endian+'II',zone,88,0,index)
            records.append(exs_record(big,1,index,name,zone))
        records.append(exs_record(big,2,0,'Keys',bytes(8)))
        for index,sample in enumerate(['soft.wav','bright.wav']):
            data = bytearray(592)
            struct.pack_into(endian+'IIIIII',data,0,44,frames,rate,16,1,1)
            data[336:336+len(sample)] = sample.encode()
            records.append(exs_record(big,3,index,sample,data))
        (root/('harbor-big.exs' if big else 'harbor.exs')).write_bytes(b''.join(records))


if __name__=='__main__':
    generate(Path(__file__).resolve().parents[1]/'crates/little-clip-core/tests/fixtures/multisample')
