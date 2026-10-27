#!/usr/bin/env python3
"""Native session columns answering questions about synthetic images."""
from pathlib import Path
from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance
from file_manager_answer_acceptance import model_environment
from file_manager_search_acceptance import submit
from search_model.quality_visuals import answer_images
from text_strip_acceptance import hover_observation


def exercise(session, root):
    session.observe('initial', ['card.png', 'shape.png'])
    session.click(530, 160)
    index = Path(session.env['XDG_DATA_HOME']) / 'little-screen-clip/search/index.sqlite'
    assert not index.exists()
    submit(session, ':q:"What numeric rating is explicitly shown?";type:number')
    session.observe('visual-number', ['18 points', 'card.png', 'shape.png', 'Session answers'])
    hover_observation(session, 'visual-evidence', 835, 346, ['model observation', 'region', '18 points'])
    hover_observation(session, 'visual-nonapplicable', 835, 455, ['Not applicable to this question'])
    submit(session, ':q:"Is the rectangle blue?";type:boolean')
    session.observe('visual-false', ['false', 'shape.png', 'Session answers'])
    assert session.word('visual-false', 'false', minimum_y=420)
    assert not index.exists(), 'Visual questions must not enroll the folder or create an index'


def main():
    repository = Path(__file__).resolve().parent.parent
    args = application_arguments(__doc__, repository / 'target/debug/clip-file-manager')
    output, root = media_fixture_directory(args)
    answer_images(root)
    (root / 'injection.png').unlink()
    return run_application_acceptance(args, output, root, exercise,
        'Native typed visual answer, explicit boolean false and no automatic indexing', model_environment)


if __name__ == '__main__':
    raise SystemExit(main())
