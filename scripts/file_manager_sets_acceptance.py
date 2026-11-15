#!/usr/bin/env python3
"""Exercise native sorting sets, subdivision, filtering and restart."""
import json
from pathlib import Path
import ui_acceptance_capture as private
from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance, wait_for
from file_manager_home_acceptance import click_word
from file_manager_session_acceptance import close_shelf


def menu(session,shot,name,filtered=False):
    click_word(session,shot,"Sets:" if filtered else "Sets",minimum_y=140,maximum_y=200)
    return session.observe(name,["Unassigned files","New root set"])


def exercise(session,root,binary):
    session.ocr_regions=[("sets-menu-ocr.png",(725,180,1030,560))]
    state=Path(session.env["XDG_STATE_HOME"])/"little-screen-clip/organization.json"
    saved=lambda:json.loads(state.read_text())["sets"] if state.exists() else {"groups":[],"members":[]}
    original={p:p.read_bytes() for p in root.glob("*.txt")}
    text=session.observe("folder",["alpha.txt","beta.txt","gamma.txt","Sets"])
    close_shelf(session,"folder",text)
    for i,(name,group) in enumerate([("alpha.txt","A"),("beta.txt","B")]):
        shot=f"before-{i}"
        session.observe(shot,[name,"Sets"])
        click_word(session,shot,name,minimum_y=230)
        session.observe(f"selected-{i}",[name,"Sets"])
        menu(session,f"selected-{i}",f"assign-menu-{i}")
        click_word(session,f"assign-menu-{i}","Assign",minimum_y=200)
        session.observe(f"assign-choice-{i}",["A","B"])
        # Captured native submenu geometry; persisted membership below verifies
        # this assignment rather than the identically named parent filter.
        session.click(1044,428+42*i)
        wait_for(lambda:len(saved()["members"])==i+1,"classification saved")
        session.key("Escape")
    session.observe("assigned",["alpha.txt","beta.txt","gamma.txt"])
    menu(session,"assigned","filter-menu")
    # The isolated one-letter filter is missed by sparse OCR in light mode.
    # Its observed row is checked by the filtered listing immediately below.
    session.click(746,332)
    session.key("Escape")
    text=session.observe("filtered-a",["alpha.txt","Sets:"])
    assert "beta.txt" not in text and "gamma.txt" not in text
    menu(session,"filtered-a","subdivide-menu",True)
    click_word(session,"subdivide-menu","Subdivide",minimum_y=200)
    session.observe("subset-editor",["Create sorting set","New subset","Save set"])
    # Captured inline Name field lies in the left side of the full-width editor.
    session.click(244,233)
    session.key("a","ctrl")
    private.run_checked(["wtype","Keep"],env=session.env)
    click_word(session,"subset-editor","Save",minimum_y=180,maximum_y=300)
    wait_for(lambda:any(g["name"]=="Keep" for g in saved()["groups"]),"subdivision saved")
    session.observe("subset-saved",["alpha.txt","Sets:"])
    click_word(session,"subset-saved","alpha.txt",minimum_y=230)
    session.observe("subset-selected",["alpha.txt","Sets:"])
    menu(session,"subset-selected","subset-assign-menu",True)
    click_word(session,"subset-assign-menu","Assign",minimum_y=200)
    session.observe("subset-choice",["Keep"])
    click_word(session,"subset-choice","Keep",minimum_y=500)
    keep=next(g["id"] for g in saved()["groups"] if g["name"]=="Keep")
    wait_for(lambda:any(m["path"]==str(root/"alpha.txt") and m["group"]==keep for m in saved()["members"]),"subdivision membership saved")
    session.key("Escape")
    text=session.observe("parent-includes-subset",["alpha.txt","Keep"])
    assert "beta.txt" not in text
    menu(session,"parent-includes-subset","unassigned-menu",True)
    click_word(session,"unassigned-menu","Unassigned",minimum_y=180)
    session.key("Escape")
    text=session.observe("unassigned",["gamma.txt"])
    assert "alpha.txt" not in text and "beta.txt" not in text
    menu(session,"unassigned","all-menu",True)
    click_word(session,"all-menu","All",minimum_y=180)
    session.key("Escape")
    session.observe("all-again",["alpha.txt","beta.txt","gamma.txt","Keep"])
    session.close(); session.restart(binary,root)
    session.observe("restarted",["alpha.txt","beta.txt","gamma.txt","Sets"])
    assert len(saved()["members"])==2
    assert next(g for g in saved()["groups"] if g["name"]=="Keep")["parent"] is not None
    for path,data in original.items(): assert path.read_bytes()==data


if __name__=="__main__":
    args=application_arguments(__doc__,Path("target/debug/clip-file-manager"))
    output,root=media_fixture_directory(args)
    for name in ["alpha","beta","gamma"]: (root/(name+".txt")).write_text(name+" original\n")
    raise SystemExit(run_application_acceptance(args,output,root,
        lambda session,root:exercise(session,root,args.binary),
        "Sorting-set assignment, parent and unassigned filters, repeated subdivision, labels and restart without moving source files"))
