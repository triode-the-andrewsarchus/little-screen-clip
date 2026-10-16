#!/usr/bin/env python3
"""PDF info/page previews through pinned inputs on private Weston/Niri."""
from pathlib import Path
from PIL import Image
from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance, wait_for, private, preserve_file_clipboard


def fixtures(root):
    stream=b'BT /F1 24 Tf 40 120 Td (Pinned document preview) Tj ET'
    objects=[b'<< /Type /Catalog /Pages 2 0 R >>',b'<< /Type /Pages /Kids [3 0 R] /Count 1 >>',b'<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 200] /Resources << /Font << /F1 5 0 R >> >> /Contents 4 0 R >>',b'<< /Length '+str(len(stream)).encode()+b' >>\nstream\n'+stream+b'\nendstream',b'<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>']
    data=bytearray(b'%PDF-1.4\n');offsets=[0]
    for i,body in enumerate(objects,1):
        offsets.append(len(data));data.extend(f'{i} 0 obj\n'.encode()+body+b'\nendobj\n')
    xref=len(data);data.extend(b'xref\n0 6\n0000000000 65535 f \n')
    for offset in offsets[1:]:data.extend(f'{offset:010} 00000 n \n'.encode())
    data.extend(f'trailer\n<< /Size 6 /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n'.encode())
    (root/'document.pdf').write_bytes(data);(root/'notes.txt').write_text('Ordinary text beside a PDF.\n')


def exercise(session,root):
    session.observe('01-pdf-strip',['document.pdf','1 page'])
    session.click(848,201)
    session.observe('02-mixed-folder',['document.pdf','notes.txt'])
    path=session.output/'03-page-raster.png'
    def painted():
        with preserve_file_clipboard(session.env):private.screenshot(session.niri,session.env,path)
        with Image.open(path) as image:
            # The page raster is white in the first strip body, distinct from dark chrome.
            pixels=image.convert('RGB').crop((420,365,490,425)).getdata()
            return sum(min(pixel)>225 for pixel in pixels)>1000
    if session.output.name.endswith('dark'):wait_for(painted,'Pinned PDF page raster')


def main():
    args=application_arguments(__doc__,Path('target/release/clip-file-manager'));output,root=media_fixture_directory(args);fixtures(root)
    return run_application_acceptance(args,output,root,exercise,'Pinned PDF metadata and page rendering in a mixed list')


if __name__=='__main__':
    raise SystemExit(main())
