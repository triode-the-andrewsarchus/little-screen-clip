#!/usr/bin/env python3
"""Search-to-studio handoff keeps the selected source through deferred PDF reads."""
import json
from pathlib import Path
import shutil

import ui_acceptance_capture as private
from ui_acceptance_fixtures import pdf_fixture
from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance, wait_for
from file_manager_offline_acceptance import chunks, payload_accesses, trace_application
from file_manager_search_acceptance import submit


def open_result(session, shot, name):
    session.click(*session.word(shot, name, minimum_y=250))
    session.key('Return')


def exercise(session, root):
    session.observe('initial', ['mesh.stl'])
    index = Path(session.env['XDG_DATA_HOME']) / 'little-screen-clip/search/index.sqlite'
    assert not index.exists()
    submit(session, ':*.stl mesh;scope:recursive')
    session.observe('model-result', ['mesh.stl', '1 matching entries'])
    open_result(session, 'model-result', 'mesh.stl')
    session.observe('checked-model-studio', ['Back to folder', '1 triangles'])
    session.click(155, 118)  # Verified Back-to-folder button in the private 1280x800 output.
    session.observe('returned-model-results', ['mesh.stl', '1 matching entries'])
    submit(session, ':*.pdf Document.pdf;scope:recursive')
    session.observe('pdf-result', ['Document.pdf', '1 matching entries'])
    open_result(session, 'pdf-result', 'Document.pdf')
    text = session.observe('checked-pdf-studio', ['Back to folder', '2 pages', 'Acceptance page 01'])
    assert 'Document.ocr.edited.pdf' not in text, 'Search must open the selected observation rather than an OCR sibling'
    offsets = {path.name: path.stat().st_size for path in session.output.glob('source-trace.*')}
    root.rename(session.output / 'original-source')
    root.mkdir()
    pdf_fixture(root, 4, 'Document.pdf')
    target = root / 'Document.pdf'
    # Force a new high-resolution source read after the preliminary open passed.
    private.run_checked([str(session.pointer), 'scroll', '1280', '800', '850', '450', '-1000'], env=session.env)
    private.run_checked([str(session.pointer), 'scroll', '1280', '800', '850', '450', '-200'], env=session.env)
    session.observe('zoom-after-source-replacement', ['Back to folder', '800%', 'Search result changed'])
    def source_checks():
        return [line for name, text in chunks(session, offsets).items() if name not in offsets
                for line in text.splitlines() if str(root) in line and 'O_PATH' in line]
    wait_for(lambda: source_checks(), 'a deferred studio worker revalidates the original source context')
    denied = payload_accesses(chunks(session, offsets), target)
    assert not denied, denied
    assert not index.exists(), 'Opening matched files must not enroll the directory'
    (session.output / 'studio-source.json').write_text(json.dumps({
        'checked_model_loaded': True, 'selected_pdf_loaded': True,
        'ocr_sibling_not_substituted': True, 'replacement_payload_accesses': len(denied),
        'deferred_source_observations': len(source_checks()), 'no_index_created': True,
    }, indent=2))


def main():
    repo = Path(__file__).resolve().parent.parent
    args = application_arguments(__doc__, repo / 'target/debug/clip-file-manager')
    output, root = media_fixture_directory(args)
    pdf_fixture(root, 2, 'Document.pdf')
    shutil.copyfile(root / 'Document.pdf', root / 'Document.ocr.edited.pdf')
    (root / 'mesh.stl').write_text('solid fixture\nfacet normal 0 0 1\nouter loop\nvertex 0 0 0\nvertex 1 0 0\nvertex 0 1 0\nendloop\nendfacet\nendsolid fixture\n')
    trace_application(args, output)
    return run_application_acceptance(args, output, root, exercise,
        'Actual checked model/PDF studio loads, exact selected PDF, and no replacement payload access during deferred zoom rendering')


if __name__ == '__main__':
    raise SystemExit(main())
