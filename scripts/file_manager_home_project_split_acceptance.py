#!/usr/bin/env python3
"""Verify explicit migration of a shared project area without changing project sources."""
import json
from pathlib import Path
from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance, wait_for
from file_manager_home_acceptance import click_word
from file_manager_home_review import create


def exercise(session, root, binary):
    session.observe("folder", ["Home"])
    session.close()
    state_home = Path(session.env["XDG_STATE_HOME"])
    desk = root / "desk"
    original = create(desk, state_home=state_home)
    home = state_home / "little-screen-clip/home.json"
    registry = state_home / "little-screen-clip/projects.json"
    legacy = original["board"]["cards"][0]
    legacy["rect"] = [620, 80, 570, 510]
    legacy["content"] = {"kind":"projects", "title":"My projects"}
    original["board"]["cards"] = [legacy]
    home.write_text(json.dumps(original))
    registry_bytes = registry.read_bytes()
    sources = {p:p.read_bytes() for p in (desk/"projects").rglob("*") if p.is_file()}
    read = lambda: json.loads(home.read_text())["board"]
    session.restart(binary)
    session.observe("legacy", ["My projects", "Split into individual panels", "Pack spare batteries"])
    assert read()["cards"] == [legacy], "opening must not silently migrate old placements"
    click_word(session, "legacy", "Split", minimum_y=170, maximum_y=350)
    wait_for(lambda:len(read()["cards"])==2 and all(c["content"]["kind"]=="project" for c in read()["cards"]), "individual panels saved")
    split = read()["cards"]
    assert {c["content"]["project"] for c in split} == {p["id"] for p in json.loads(registry_bytes)["projects"]}
    assert split[0]["id"] != split[1]["id"]
    session.observe("split", ["Field recorder", "Courtyard garden", "Pack spare batteries"])
    assert all(p.read_bytes()==content for p,content in sources.items())
    todo = desk/"projects/field-recorder/TODO.md"
    click_word(session, "split", "Pack", minimum_y=150)
    checked = sources[todo].replace(b"[ ] Pack", b"[x] Pack")
    wait_for(lambda:todo.read_bytes()==checked, "split panel still edits real TODO")
    session.observe("checked", ["Field recorder", "Courtyard garden"])
    click_word(session, "checked", "Undo", minimum_y=90, maximum_y=145, control_bounds=(650,103,705,134))
    wait_for(lambda:read()["cards"]==[legacy], "one undo restores shared project area")
    session.observe("undo", ["My projects", "Split into individual panels"])
    assert todo.read_bytes()==checked, "layout undo cannot undo source tasks"
    click_word(session, "undo", "Redo", minimum_y=90, maximum_y=145, control_bounds=(714,103,770,134))
    wait_for(lambda:read()["cards"]==split, "redo restores same individual placement identities")
    session.observe("redo", ["Field recorder", "Courtyard garden"])
    session.close();session.restart(binary)
    session.observe("restart", ["Field recorder", "Courtyard garden", "Pack spare batteries"])
    assert read()["cards"]==split
    assert read()["drawing"]==original["board"]["drawing"]
    assert registry.read_bytes()==registry_bytes
    assert all(p.read_bytes()==(checked if p==todo else content) for p,content in sources.items())


if __name__=="__main__":
    args=application_arguments(__doc__,Path("target/debug/clip-file-manager"))
    output,root=media_fixture_directory(args)
    raise SystemExit(run_application_acceptance(args,output,root,
        lambda session,root:exercise(session,root,args.binary),
        "Explicit project-area split, independent placements, real task write, layout undo/redo and restart"))
