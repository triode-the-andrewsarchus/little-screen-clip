#!/usr/bin/env python3
"""Review compact page metadata on the file manager's private desktop."""
from pathlib import Path

from file_manager_acceptance import (
    application_arguments,
    media_fixture_directory,
    run_application_acceptance,
)

PAGES = {
    "a-yaml.md": "---\ntitle: YAML article\ndate: 2026-11-12\ndraft: true\ntags: [rust, diagrams]\n---\n\n# Body\n\nSome **rendered content**.\n",
    "b-toml.md": "+++\ntitle = 'TOML article'\ndate = 2026-11-12\ndraft = false\ntags = ['rust', 'metadata']\n+++\n\n# Body\n\nA typed date.\n",
    "c-json.md": '{"title":"JSON article","date":"2026-11-12","draft":true,"tags":["json","publishing"]}\n\n# Body\n\nA JSON page.\n',
}


def exercise(session, root):
    session.observe(
        "01-metadata-strips",
        ["YAML article", "TOML article", "JSON article", "2026-11-12", "draft", "tags"],
    )
    # Verified toolbar button in the runner's fixed 1280×800 viewport.
    session.click(650, 160)
    session.observe("02-expanded-list", ["YAML article", "TOML article", "JSON article", "draft", "publishing"])
    for name, source in PAGES.items():
        assert (root / name).read_text() == source, "passive previews must not rewrite front matter"


def main():
    args = application_arguments(__doc__, Path("target/debug/clip-file-manager"))
    output, root = media_fixture_directory(args)
    for name, source in PAGES.items():
        (root / name).write_text(source)
    return run_application_acceptance(args, output, root, exercise, "page metadata strips")


if __name__ == "__main__":
    raise SystemExit(main())
