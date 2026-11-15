#!/usr/bin/env python3
"""Exercise real freehand region input, exact hit areas and guarded actions."""
from pathlib import Path
import json
from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance, wait_for
from file_manager_home_acceptance import click_word, click_cropped_word, enter_home
import ui_acceptance_capture as private


def trace(session, points):
    private.run_checked([str(session.pointer), "right-trace", *map(str, session.logical_size),
        *[str(value) for point in points for value in point]], env=session.env)


def save(session, name, target, step):
    session.observe(step, ["choose an action"])
    x,y = session.word(step, "Name", tree=True)
    session.click(x+150,y)
    session.key("a", "ctrl")
    private.run_checked(["wtype",name], env=session.env)
    x,y = session.word(step, "Absolute")
    session.click(x+120,y)
    private.run_checked(["wtype",str(target)], env=session.env)
    session.observe(step+"-target", [name, "Open folder"])
    x,y = session.word(step+"-target", "Open", tree=True, minimum_y=240,last=True)
    if not click_cropped_word(session,step+"-target","Save",(100,y+15,212,y+58),140,400):
        # The observed shared primary button is immediately below the target
        # summary; colored button fills can make OCR omit its label entirely.
        session.click(x+45,y+34)


def exercise(session, root, binary):
    first,second = root/"NotesRoom",root/"OtherRoom"
    first.mkdir(); second.mkdir()
    (first/"first-marker.txt").write_text("first source")
    (second/"second-marker.txt").write_text("second source")
    state = Path(session.env["XDG_STATE_HOME"])/"little-screen-clip/home.json"
    read = lambda: json.loads(state.read_text())
    session.observe("folder",["Home"])
    enter_home(session,"folder")
    session.observe("home",["Make this place yours"])
    trace(session,[(700,370),(900,370),(900,430),(760,430),(760,580),(700,580),(700,370)])
    save(session,"Notes door",first,"first-draft")
    wait_for(lambda: state.exists() and len(read()["board"].get("regions",[]))==1,"first region committed")
    session.observe("first-saved",["Pinned references:"])
    assert not read()["board"]["cards"],"actions must not create shortcut cards"
    session.click(850,500)
    session.observe("concave-empty-corner",["Pinned references:"])
    trace(session,[(710,385),(790,385),(790,530),(710,530),(710,385)])
    save(session,"Other door",second,"second-draft")
    wait_for(lambda: len(read()["board"]["regions"])==2,"second region committed")
    session.observe("both-saved",["Pinned references:"])
    session.click(730,400)
    session.observe("overlap-choice",["Overlapping click regions","Notes door","Other door"])
    click_word(session,"overlap-choice","Other",minimum_y=145)
    session.observe("second-target",["second-marker"])
    session.key("Left","alt")
    session.observe("home-return",["Pinned references:"])
    click_word(session,"home-return","Click",minimum_y=90,maximum_y=145,control_bounds=(700,98,812,138))
    session.observe("manager",["Notes door","Other door","Delete region"])
    click_word(session,"manager","Other",minimum_y=155)
    wait_for(lambda: not next(r for r in read()["board"]["regions"] if r["label"]=="Other door")["enabled"],"region disabled")
    session.observe("disabled",["Hide regions"])
    click_word(session,"disabled","Delete",minimum_y=155)
    wait_for(lambda: len(read()["board"]["regions"])==1,"region removed")
    session.observe("deleted",["Notes door","Hide regions"])
    click_word(session,"deleted","Undo",minimum_y=90,maximum_y=145)
    wait_for(lambda: len(read()["board"]["regions"])==2,"region deletion undone")
    session.observe("undo-delete",["Notes door","Other door","Hide regions"])
    click_word(session,"undo-delete","Hide",minimum_y=145)
    session.observe("manager-hidden",["Pinned references:"])
    session.click(720,500)
    session.observe("first-target",["first-marker"])
    session.key("Left","alt")
    session.observe("before-restart",["Pinned references:"])
    original = read()["board"]["regions"]
    session.close(); session.restart(binary)
    session.observe("restarted",["Pinned references:"])
    assert read()["board"]["regions"]==original
    session.click(720,500)
    session.observe("restored-action",["first-marker"])
    assert (first/"first-marker.txt").read_text()=="first source"
    assert (second/"second-marker.txt").read_text()=="second source"


if __name__ == "__main__":
    args = application_arguments(__doc__,Path("target/debug/clip-file-manager"))
    output,root = media_fixture_directory(args)
    raise SystemExit(run_application_acceptance(args,output,root,
        lambda session,root:exercise(session,root,args.binary),
        "Native concave right-button trace, target picker, overlaps, manager, disable and restart"))
