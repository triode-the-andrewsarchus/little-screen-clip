"""Owned synthetic source files for retrieval evaluation; no user content is used."""
from pathlib import Path
import subprocess
import textwrap
from reportlab.pdfgen import canvas
from reportlab.lib.pagesizes import A4
from quality_visuals import render


BACKGROUND = 'The archive register records numbered storage bins. Each shelf has a location code, and staff check the quantities against the previous inventory. '


def make_pdf(path, body, long=False):
    pdf=canvas.Canvas(str(path),pagesize=A4, pageCompression=0)
    for text in ([BACKGROUND*5]*15 if long else [])+[body]:
        paragraph=pdf.beginText(40,790)
        paragraph.setFont('Helvetica',12)
        for line in textwrap.wrap(text,85):
            paragraph.textLine(line)
        pdf.drawText(paragraph)
        pdf.showPage()
    pdf.save()


def build(cases, root):
    root.mkdir(parents=True)
    for ordinal, case in enumerate(cases):
        extension={'text':'txt','pdf':'pdf','image':'png','audio':'wav','video':'mp4'}[case['kind']]
        case['file']=f'item_{ordinal:03}.{extension}'
        path=root/case['file']
        long=case['id'] in {'text-01','pdf-01','video-11'}
        if case['kind']=='text':
            path.write_text((BACKGROUND*45+'\n\n' if long else '')+case['body'])
        elif case['kind']=='pdf':
            make_pdf(path,case['body'],long)
            if long:
                case['expected_location_contains']='page 16'
        elif case['kind']=='image':
            render(case['scene']).save(path)
        elif case['kind']=='audio':
            subprocess.run(['espeak-ng','-v','en-us','-s','145','-w',str(path),case['body']],check=True,stdout=subprocess.DEVNULL,stderr=subprocess.PIPE,timeout=30)
        else:
            picture=path.with_suffix('.source.png')
            render(case['scene']).save(picture)
            inputs=['-loop','1','-framerate','1','-i',str(picture)]
            duration='1'
            filtering=[]
            if long:
                inputs=['-f','lavfi','-i','color=c=white:s=512x512:r=1:d=65',*inputs]
                filtering=['-filter_complex',"[0:v][1:v]overlay=enable='gte(t,60)'",'-map','0:a?']
                duration='65'
                case['expected_time_range']=[60,65]
            subprocess.run(['ffmpeg','-v','error','-y',*inputs,*filtering,'-t',duration,'-an','-c:v','libx264','-threads','1','-pix_fmt','yuv420p',str(path)],check=True,stdout=subprocess.DEVNULL,stderr=subprocess.PIPE,timeout=60)
            picture.unlink()
    return cases
