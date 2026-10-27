#!/usr/bin/env python3
"""Synthetic retrieval-quality component: production readers and model inputs, no index enrollment.

This evaluates per-file maximum semantic scores, not the application's lexical/RRF
fusion, coordinator lifecycle or UI. It does not replace end-to-end acceptance.
"""
import argparse
import base64
import hashlib
import json
import math
import os
from pathlib import Path
import select
import signal
import struct
import subprocess
import sys
import time
from quality_fixtures import build
from smoke import vectors

HERE=Path(__file__).resolve().parent
DIMENSION=768



def answer_check_arguments(description):
    parser = argparse.ArgumentParser(description=description)
    parser.add_argument('--runtime', type=Path, required=True)
    parser.add_argument('--models', type=Path, required=True)
    parser.add_argument('--device', default='cuda')
    parser.add_argument('--output-dir', type=Path, required=True)
    args = parser.parse_args()
    output = args.output_dir.resolve()
    output.mkdir(parents=True, exist_ok=False)
    return args, output


def write_check_report(worker, checks, started, output, scope):
    report = {'model':worker.ready, 'seconds':time.monotonic()-started,
              'checks':checks, 'passed':sum(check['passed'] for check in checks),
              'total':len(checks), 'scope':scope}
    (output / 'report.json').write_text(json.dumps(report, indent=2))
    return int(report['passed'] != report['total'])


class Worker:
    def __init__(self,args,output,extra_args=(),on_spawn=None):
        self.log=(output/'model.stderr.log').open('wb')
        self.process=subprocess.Popen([sys.executable,'-I',str(HERE/'worker.py'),'--runtime',str(args.runtime),'--models',str(args.models),'--device',args.device,*extra_args],stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=self.log,start_new_session=True)
        self.buffer=bytearray()
        self.sequence=0
        self.request_hashes=[]
        try:
            if on_spawn:
                on_spawn(self.process.pid)
            self.ready=self.read(90)
            if not self.ready.get('ready'):
                raise RuntimeError(self.ready)
        except BaseException:
            self.close()
            raise

    def read(self,timeout):
        deadline=time.monotonic()+timeout
        while b'\n' not in self.buffer:
            remaining=deadline-time.monotonic()
            if remaining<=0 or not select.select([self.process.stdout],[],[],remaining)[0]:
                raise TimeoutError('Model worker response deadline expired')
            data=os.read(self.process.stdout.fileno(),65536)
            if not data:
                raise RuntimeError('Model worker closed its output')
            self.buffer.extend(data)
            if len(self.buffer)>128*1024:
                raise RuntimeError('Model response exceeds quality-runner limit')
        line,_,self.buffer=self.buffer.partition(b'\n')
        return json.loads(line)

    def request(self,request):
        fingerprint=hashlib.sha256(json.dumps(request,sort_keys=True,separators=(',',':'),allow_nan=False).encode()).hexdigest()
        self.sequence+=1
        request={**request,'id':self.sequence}
        self.process.stdin.write((json.dumps(request)+'\n').encode())
        self.process.stdin.flush()
        response=self.read(90)
        if response.get('id')!=self.sequence or not response.get('ok'):
            raise RuntimeError(response)
        self.request_hashes.append(fingerprint)
        return response

    def close(self):
        if self.process.poll() is None:
            os.killpg(self.process.pid,signal.SIGKILL)
        self.process.wait()
        self.log.close()


def reader(binary,request):
    result=subprocess.run([str(binary.resolve()),'--internal-file-search-reader'],input=json.dumps(request).encode(),capture_output=True,timeout=120,check=True)
    response=json.loads(result.stdout)
    if 'Err' in response:
        raise RuntimeError(response['Err'])
    return response['Ok']


def prepared(binary,root):
    origin=reader(binary,{'Probe':str(root)})['Root']
    listing=reader(binary,{'List':{'root':origin,'path':str(root),'hidden':False,'exclusions':[]}})['Directory']
    if listing['skipped']:
        raise RuntimeError('Fixture inventory is incomplete')
    for entry in listing['entries']:
        cursor=0
        while True:
            item=reader(binary,{'Extract':{'root':origin,'entry':entry,'cursor':cursor}})['Prepared']
            yield entry['name'],item
            if item['next'] is None:
                break
            if item['next']<=cursor:
                raise RuntimeError('Reader did not advance')
            cursor=item['next']


def embed_prepared(worker,item):
    if item['data']:
        yield vectors(worker.request({'op':item['modality'],'data':item['data'],'budget':item.get('vision_tokens') or 280}))[0],item['location'],item['text']
        return
    text=item['text'].encode()
    start=0
    while start<len(text):
        limit=min(len(text),start+8192)
        while limit>start:
            try:
                chunk=text[start:limit].decode()
                break
            except UnicodeDecodeError:
                limit-=1
        passage=worker.request({'op':'passage','text':chunk})
        end=passage['end'];advance=passage['next']
        if not 0<advance<=end<=len(chunk.encode()):
            raise RuntimeError('Tokenizer did not produce valid passage offsets')
        content=text[start:start+end].decode()
        response=worker.request({'op':'text','inputs':['title: none | text: '+content],'token_limit':2048})
        yield vectors(response)[0],f"{item['location']} · bytes {start}..{start+end}",content
        start=start+end if start+end==len(text) else start+advance


def half_vector(vector):
    norm=math.sqrt(sum(x*x for x in vector))
    normalized=struct.unpack('<768f',struct.pack('<768f',*(x/norm for x in vector)))
    rounded=struct.unpack('<768e',struct.pack('<768e',*normalized))
    norm=math.sqrt(sum(x*x for x in rounded))
    return struct.unpack('<768f',struct.pack('<768f',*(x/norm for x in rounded)))


def scores(query,segments,storage):
    files={}
    for segment in segments:
        vector=segment[storage]
        score=sum(a*b for a,b in zip(query,vector))
        key=segment['file']
        previous=files.get(key)
        if previous is None or score>previous['score']:
            files[key]={'file':key,'score':score,'location':segment['location'],'modality':segment['modality']}
    return sorted(files.values(),key=lambda row:(-row['score'],row['file']))


def evaluate(cases,negatives,queries,segments,storage,floor):
    checks=[]
    for case in cases:
        ranked=[row for row in scores(queries[case['id']],segments,storage) if row['score']>=floor.get(row['modality'],0.25)]
        rank=next((i+1 for i,row in enumerate(ranked) if row['file']==case['file']),None)
        expected=case.get('expected_location_contains')
        target=next((row for row in ranked if row['file']==case['file']),None)
        time_range=case.get('expected_time_range')
        location_ok=not expected or bool(target and expected in target['location'])
        if time_range:
            location_ok=bool(target and time_range[0]<=float(target['location'].rsplit(' ',1)[-1])<=time_range[1])
        checks.append({'id':case['id'],'kind':case['kind'],'split':case['split'],'rank':rank,'recall_at_10':rank is not None and rank<=10,'location_passed':location_ok,'target':target,'top':ranked[:10]})
    noise=[]
    for case in negatives:
        ranked=[row for row in scores(queries[case['id']],segments,storage) if row['score']>=floor.get(row['modality'],0.25)]
        noise.append({'id':case['id'],'split':case['split'],'semantic_match':bool(ranked),'top':ranked[:3]})
    per_kind={kind:sum(row['recall_at_10'] for row in checks if row['kind']==kind)/sum(row['kind']==kind for row in checks) for kind in sorted({row['kind'] for row in checks})}
    return {'storage':storage,'floors':floor,'recall_at_10':sum(row['recall_at_10'] for row in checks)/len(checks),'per_kind':per_kind,'negative_match_rate':sum(row['semantic_match'] for row in noise)/len(noise),'checks':checks,'negatives':noise}


def calibration_floors(cases,negatives,queries,segments):
    # Maximum-margin midpoint uses only the declared calibration split. The
    # report retains validation failures; this never rewrites app preferences.
    positive= {mod:[] for mod in ['text','image','audio']}
    negative= {mod:[] for mod in ['text','image','audio']}
    for case in cases:
        if case['split']!='calibration':
            continue
        target=next(row for row in scores(queries[case['id']],segments,'f16') if row['file']==case['file'])
        positive[target['modality']].append(target['score'])
    for case in negatives:
        if case['split']!='calibration':
            continue
        for row in scores(queries[case['id']],segments,'f16'):
            negative[row['modality']].append(row['score'])
    return {mod:round((min(positive[mod])+max(negative[mod]))/2,2) for mod in positive}


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--runtime',type=Path,required=True)
    parser.add_argument('--models',type=Path,required=True)
    parser.add_argument('--binary',type=Path,required=True)
    parser.add_argument('--device',default='cuda',choices=['cuda','vulkan','cpu'])
    parser.add_argument('--output-dir',type=Path,required=True)
    args=parser.parse_args()
    output=args.output_dir.resolve()
    output.mkdir(parents=True,exist_ok=False)
    frozen=(HERE/'retrieval_cases.json').read_bytes()
    corpus=json.loads(frozen)
    cases=build(corpus['positives'],output/'fixture')
    (output/'fixture-manifest.json').write_text(json.dumps(corpus,indent=2))
    validation_raw=(HERE/'retrieval_validation.json').read_bytes()
    validation=json.loads(validation_raw)
    targets={case['id']:case for case in cases}
    validation_cases=[{**targets[row['target']],**row} for row in validation['positives']]
    started=time.monotonic()
    worker=Worker(args,output)
    segments=[]
    queries={}
    try:
        for name,item in prepared(args.binary,output/'fixture'):
            for vector,location,text in embed_prepared(worker,item):
                segments.append({'text':text,'file':name,'modality':item['modality'],'location':location,'f32':vector,'f16':half_vector(vector)})
            print(json.dumps({'file':name,'segments':len(segments)}),flush=True)
        for case in cases+corpus['negatives']+validation_cases+validation['negatives']+corpus['exact_queries']:
            queries[case['id']]=vectors(worker.request({'op':'text','inputs':['task: search result | query: '+case['query']],'token_limit':512}))[0]
        report={'scope':__doc__,'scoring':'common float64 dot accumulation for both storage comparisons; not bit-identical to Rust float32 accumulation','source_sha256':{name:hashlib.sha256((HERE/name).read_bytes()).hexdigest() for name in ['quality.py','quality_fixtures.py','quality_visuals.py','worker.py']},'fixture_sha256':hashlib.sha256(frozen).hexdigest(),'pipeline':worker.ready,'seconds':time.monotonic()-started,'segment_count':len(segments),'positive_count':len(cases),'negative_count':len(corpus['negatives']),'results':[evaluate(cases,corpus['negatives'],queries,segments,storage,{'text':0.25,'image':0.25,'audio':0.25}) for storage in ['f32','f16']],'full_phase6_acceptance_proven':False,'bf16_comparison_proven':False,'exact_filename_tier_proven':False}
        candidate=calibration_floors(cases,corpus['negatives'],queries,segments)
        report['calibration_method']='Midpoint between weakest calibration positive and strongest calibration negative per modality, rounded to 0.01; exploratory initial hold-out was previously inspected; additional validation frozen before evaluating these candidates.'
        report['candidate_floors']=candidate
        report['calibrated_results']=[evaluate(cases,corpus['negatives'],queries,segments,storage,candidate) for storage in ['f32','f16']]
        report['validation_sha256']=hashlib.sha256(validation_raw).hexdigest()
        report['validation']={str(floors):evaluate(validation_cases,validation['negatives'],queries,segments,'f16',floors) for floors in [{'text':.25,'image':.25,'audio':.25},candidate]}
        report['application_defaults_changed']=False
        report['model_request_sha256']=worker.request_hashes
        (output/'vectors.json').write_text(json.dumps({'segments':segments,'queries':queries}))
        (output/'report.json').write_text(json.dumps(report,indent=2))
        print(json.dumps({key:report[key] for key in ['positive_count','negative_count','segment_count','seconds']}),flush=True)
        for result in report['results']+report['calibrated_results']+list(report['validation'].values()):
            print(json.dumps({key:result[key] for key in ['storage','recall_at_10','per_kind','negative_match_rate']}),flush=True)
    except BaseException as error:
        (output/'failure.json').write_text(json.dumps({'error':str(error),'completed_segments':len(segments),'completed_queries':len(queries),'full_phase6_acceptance_proven':False},indent=2))
        raise
    finally:
        worker.close()


if __name__=='__main__':
    main()
