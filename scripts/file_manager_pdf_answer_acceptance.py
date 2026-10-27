#!/usr/bin/env python3
"""Native typed answers from short, scanned and conflicting-page synthetic PDFs."""
from pathlib import Path
import subprocess
from PIL import Image, ImageDraw, ImageFont
from reportlab.pdfgen.canvas import Canvas
from reportlab.lib.utils import ImageReader
from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance
from file_manager_answer_acceptance import model_environment
from file_manager_search_acceptance import submit
from text_strip_acceptance import hover_observation


def page_image(text):
    image = Image.new('RGB', (800, 400), 'white')
    draw = ImageDraw.Draw(image)
    font = ImageFont.truetype('/usr/share/fonts/TTF/DejaVuSans.ttf', 38)
    for index, line in enumerate(text.splitlines()):
        draw.text((40, 50 + index*85), line, fill='black', font=font)
    return image


def scanned(path, texts):
    pdf = Canvas(str(path), pagesize=(800,400))
    for text in texts:
        pdf.drawImage(ImageReader(page_image(text)), 0, 0, width=800, height=400)
        pdf.showPage()
    pdf.save()


def exercise(session, root):
    session.observe('initial', ['Conflict.pdf', 'Late.pdf'])
    session.click(530, 160)
    index = Path(session.env['XDG_DATA_HOME']) / 'little-screen-clip/search/index.sqlite'
    submit(session, ':q:"What numeric rating is explicitly listed?";type:number')
    session.observe('reconciled', ['Ambiguous', '18 points', 'Conflict.pdf', 'Late.pdf'], timeout_seconds=120)
    # PDF rows share the same answer cell/header positions as other list previews.
    _, conflict_y = session.word('reconciled', 'Conflict.pdf', minimum_y=300)
    hover_observation(session, 'competing-pages', 835, conflict_y, ['page 1', 'page 2', 'model observation'])
    session.address(root / 'Short')
    session.observe('short-folder', ['Short.pdf'])
    submit(session, ':q:"What numeric rating is explicitly listed?";type:number')
    session.observe('short-answer', ['Short.pdf', '18 points'], timeout_seconds=60)
    _, short_y = session.word('short-answer', 'Short.pdf', minimum_y=300)
    hover_observation(session, 'short-page-evidence', 835, short_y, ['page 1', 'model observation'])
    session.address(root / 'Mixed')
    session.observe('mixed-folder', ['Mixed.pdf'])
    submit(session, ':q:"What numeric rating is explicitly listed?";type:number')
    session.observe('mixed-answer', ['Mixed.pdf', '18 points'], timeout_seconds=90)
    _, mixed_y = session.word('mixed-answer', 'Mixed.pdf', minimum_y=300)
    hover_observation(session, 'mixed-page-evidence', 835, mixed_y, ['page 2', 'model observation'])
    assert not index.exists(), 'PDF Q/A must not enroll folders or create a semantic index'


def main():
    repository = Path(__file__).resolve().parent.parent
    args = application_arguments(__doc__, repository / 'target/debug/clip-file-manager')
    output, root = media_fixture_directory(args)
    scanned(root / 'Conflict.pdf', ['Specification A\nRating: 14 points.', 'Specification B\nRating: 18 points.'])
    scanned(root / 'Late.pdf', ['Field notes\nA path passes trees and a pond.', 'Final assessment\nRating: 18 points.'])
    (root / 'Short').mkdir()
    pdf = Canvas(str(root / 'Short/Short.pdf'), pagesize=(800,400))
    pdf.setFont('Helvetica', 38)
    pdf.drawString(40, 220, 'Rating: 18 points.')
    pdf.save()
    (root / 'Mixed').mkdir()
    mixed = root / 'Mixed/Mixed.pdf'
    pdf = Canvas(str(mixed), pagesize=(800,400))
    pdf.setFont('Helvetica', 14)
    for line in range(8):
        pdf.drawString(40, 350-line*35, 'Field notes: the path passes trees, flowers and a quiet pond.')
    pdf.showPage()
    pdf.drawImage(ImageReader(page_image('Final assessment\nRating: 18 points.')), 0, 0, width=800, height=400)
    pdf.save()
    text = subprocess.run(['pdftotext','-f','1','-l','1',str(mixed),'-'],
        capture_output=True, text=True, check=True, timeout=30).stdout
    assert sum(not char.isspace() for char in text) >= 200
    return run_application_acceptance(args, output, root, exercise,
        'Short and mixed PDFs, late scanned-page answer, cross-page ambiguity and page evidence without indexing', model_environment)


if __name__ == '__main__':
    raise SystemExit(main())
