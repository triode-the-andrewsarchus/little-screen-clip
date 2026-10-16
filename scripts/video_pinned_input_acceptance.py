#!/usr/bin/env python3
"""Normal-size video preview with a pinned input, on private Weston/Niri."""
from pathlib import Path
import subprocess
from PIL import Image, ImageChops
from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance, private, preserve_file_clipboard, wait_for


def fixtures(root):
    subprocess.run(['ffmpeg','-v','error','-f','lavfi','-i','color=c=red:s=320x240:r=10','-t','1','-c:v','libx264','-threads','2','-pix_fmt','yuv420p',str(root/'clip.mp4')],check=True)
    (root/'notes.txt').write_text('Ordinary text beside the pinned video preview.\n')


def exercise(session,root):
    session.observe('01-video-row',['clip.mp4','H.264'])
    session.click(848,201)
    session.observe('02-mixed-row',['clip.mp4','H.264','notes.txt'])
    path=session.output/'03-painted-timeline.png'
    def painted():
        with preserve_file_clipboard(session.env):
            private.screenshot(session.niri,session.env,path)
        with Image.open(path) as image:
            rgb=image.convert('RGB');red,green,blue=rgb.split()
            mask=ImageChops.subtract(red,ImageChops.lighter(green,blue)).point(lambda value:255 if value>100 else 0)
            data=mask.tobytes()
            return max(sum(value>0 for value in data[y*image.width:(y+1)*image.width]) for y in range(image.height))>400
    wait_for(painted,'Pinned video timeline painting')


def main():
    args=application_arguments(__doc__,Path('target/release/clip-file-manager'))
    output,root=media_fixture_directory(args);fixtures(root)
    return run_application_acceptance(args,output,root,exercise,'Pinned native video timeline on a normal-size mixed file list')


if __name__=='__main__':
    raise SystemExit(main())
