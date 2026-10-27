#!/usr/bin/env python3
"""Owned channel-edit input survives original replacement; undo restores the original guard."""
import json
import ui_acceptance_capture as private
import file_manager_audio_studio_source_acceptance as base



def play_fresh_audio(session, description):
    previous_size = base.audio_tail(session)[1]
    session.key('space')
    def heard():
        values, size = base.audio_tail(session)
        return size > previous_size + 32000 and any(abs(value) > 100 for value in values)
    base.wait_for(heard, description)
    session.key('space')


def exercise(session, root, *, range_deletion=False):
    index = base.open_checked_audio(session, root)
    if range_deletion:
        session.observe('range-canvas', ['No region selected'])
        private.run_checked([str(session.pointer), 'drag', '1280', '800',
                             '360', '540', '560', '540'], env=session.env)
        session.observe('selected-range', ['Selection info:'], expected_any=['Duration', 'duration'])
        session.key('Delete')
        session.observe('derived-ready', ['Deleted', 'undo with Ctrl+Z'])
    else:
        session.key('c')
        session.observe('channel-tools', ['Apply to'])
        session.click(220, 386)  # Track, before any status toast changes the layout.
        session.key('2')
        session.observe('derived-ready', ['Channel edit applied', 'undo with Ctrl+Z'])
    replacement, offsets = base.replace_audio_root(session, root)
    play_fresh_audio(session, 'owned derivative plays after the original root is replaced')
    session.key('z', 'ctrl')
    session.key('space')
    session.observe('undo-rejects-replacement', ['Search result changed'])
    if range_deletion:
        session.key('Delete')
        session.observe('delete-rejects-replacement', ['Could not apply range deletion', 'Search result changed', 'Audio analysis unavailable'])
    session.key('y', 'ctrl')
    play_fresh_audio(session, 'redo restores the owned derivative and its read context')
    denied = base.payload_accesses(base.chunks(session, offsets), replacement)
    assert not denied, denied
    assert not index.exists()
    session.click(160, 118)  # Back to folder must not silently discard the owned edit.
    session.observe('discard-audio', ['Unsaved edits', 'Keep editing', 'Discard edits'])
    session.click(*session.word('discard-audio', 'Discard', minimum_y=300))
    session.observe('discarded-audio', ['Folders', 'tone.wav'])
    report_name = 'range-undo-source.json' if range_deletion else 'channel-undo-source.json'
    (session.output / report_name).write_text(json.dumps({
        'operation': 'range deletion' if range_deletion else 'channel edit',
        'derived_playback_after_original_replacement':True,
        'undo_rejected_replacement':True, 'redo_plays_derived_input':True,
        'replacement_payload_accesses':len(denied), 'no_index_created':True,
        'explicit_discard_required':True,
    }, indent=2))


if __name__ == '__main__':
    raise SystemExit(base.main(exercise, 'Owned channel input, original-context undo and derivative redo with zero replacement payload reads'))
