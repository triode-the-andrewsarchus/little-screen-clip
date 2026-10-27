#!/usr/bin/env python3
"""Owned range deletion, checked undo/redo, and rejection of replaced input."""
from functools import partial
import file_manager_audio_studio_source_acceptance as base
from file_manager_channel_undo_source_acceptance import exercise


if __name__ == '__main__':
    raise SystemExit(base.main(partial(exercise, range_deletion=True),
        'Range deletion, guarded undo/redo and explicit discard with zero replacement payload reads'))
