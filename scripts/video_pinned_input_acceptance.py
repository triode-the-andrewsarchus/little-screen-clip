#!/usr/bin/env python3
"""Normal-size video preview with a pinned input, on private Weston/Niri."""
from pathlib import Path
import subprocess
from PIL import Image, ImageChops
from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance, private, preserve_file_clipboard, wait_for


def fixtures(root, format):
    codec = 'mpeg2video' if format == 'mxf' else 'libx264'
    subprocess.run(['ffmpeg','-nostdin','-v','error','-f','lavfi','-i','color=c=red:s=320x240:r=25','-t','1','-c:v',codec,'-threads','2','-pix_fmt','yuv420p',str(root/f'clip.{format}')],check=True)
    (root/'notes.txt').write_text('Ordinary text beside the pinned video preview.\n')


def exercise(session,root):
    filename = next(root.glob('clip.*')).name
    codec = 'MPEG-2' if filename.endswith('.mxf') else 'H.264'
    session.observe('01-video-row',[filename,codec])
    session.click(848,201)
    session.observe('02-mixed-row',[filename,codec,'notes.txt'])
    path=session.output/'03-painted-timeline.png'
    def painted():
        path.unlink(missing_ok=True)
        with preserve_file_clipboard(session.env):
            private.screenshot(session.niri,session.env,path)
        with Image.open(path) as image:
            rgb=image.convert('RGB');red,green,blue=rgb.split()
            mask=ImageChops.subtract(red,ImageChops.lighter(green,blue)).point(lambda value:255 if value>100 else 0)
            data=mask.tobytes()
            return max(sum(value>0 for value in data[y*image.width:(y+1)*image.width]) for y in range(image.height))>400
    wait_for(painted,'Pinned video timeline painting')
    cache_root = Path(session.env.get('XDG_CACHE_HOME', str(Path(session.env['HOME']) / '.cache')))
    cache = cache_root / 'little-screen-clip/artifacts/video-timelines-v4'
    wait_for(lambda: list(cache.glob('*.jpg')), 'completed timeline cache')
    with Image.open(next(cache.glob('*.jpg'))) as image:
        assert all(r > 230 and g < 20 and b < 20 for r,g,b in image.convert('RGB').get_flattened_data()), 'Completed solid-red timeline has missing or corrupt columns'
    session.observe('04-completed-strip', [filename,codec])


def main():
    args=application_arguments(__doc__,Path('target/release/clip-file-manager'),lambda parser: parser.add_argument('--format',choices=['mp4','mxf'],default='mp4'))
    output,root=media_fixture_directory(args);fixtures(root,args.format)
    return run_application_acceptance(args,output,root,exercise,'Pinned native video timeline on a normal-size mixed file list')


if __name__=='__main__':
    raise SystemExit(main())
