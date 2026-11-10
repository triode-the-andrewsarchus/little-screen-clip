#!/usr/bin/env python3
"""Generate owned comic pages with deliberately shuffled numeric filenames."""
from io import BytesIO
from pathlib import Path
from zipfile import ZipFile, ZIP_DEFLATED
from PIL import Image, ImageDraw, ImageFont


def make(path):
    font = ImageFont.truetype('DejaVuSans.ttf', 32)
    with ZipFile(path, 'w', compression=ZIP_DEFLATED) as archive:
        for number, color in [(10, (40, 80, 220)), (2, (30, 170, 70)), (1, (220, 60, 40))]:
            page = Image.new('RGB', (400, 600), 'white')
            draw = ImageDraw.Draw(page)
            draw.text((25, 20), f'PAGE {number}', font=font, fill='black')
            draw.text((25, 55), {1: 'RED STORY', 2: 'GREEN STORY', 10: 'BLUE STORY'}[number], font=font, fill='black')
            draw.rectangle((25, 90, 375, 370), fill=color)
            draw.rectangle((25, 390, 180, 570), outline=color, width=8)
            draw.rectangle((200, 390, 375, 570), fill=color)
            buffer = BytesIO()
            page.save(buffer, format='PNG')
            archive.writestr(f'page{number}.png', buffer.getvalue())
        archive.writestr('__MACOSX/._page0.png', b'not an image')
        archive.writestr('ComicInfo.xml', '<ComicInfo><Title>Owned comic fixture</Title></ComicInfo>')


if __name__ == '__main__':
    make(Path(__file__).resolve().parents[1] / 'crates/clip-file-manager/tests/fixtures/comic.cbz')
