#!/usr/bin/env python3
"""Exercise native session annotations without changing any source files."""
import json
from pathlib import Path
from PIL import Image
import ui_acceptance_capture as private
from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance, wait_for
from file_manager_home_acceptance import click_word


def open_session(session, shot, name, enabled=True):
    click_word(session, shot, "Session:" if enabled else "Session", minimum_y=140, maximum_y=270)
    return session.observe(name, ["Enable session marks", "stored marks", "Session name and color meanings"])


def line_count(session, shot, rgb):
    with Image.open(session.output / f"{shot}.png").convert("RGB") as image:
        pixels = image.load()
        return sum(sum(all(abs(value-target)<=2 for value,target in zip(pixels[x,y],rgb))
                       for x in range(340,1180))>=60 for y in range(230,730))


def close_shelf(session, shot, text):
    if "hands" in text.lower() and "collected" in text.lower():
        point=session.word(shot,"Shelf(0",required=False,minimum_y=140)
        # Captured Shelf(0) toolbar control in the fixed private-runner geometry.
        session.click(*(point if point else (600,160)))


def exercise(session, root, binary, appearance):
    session.ocr_regions=[("contrast-line-org-error-ocr.png",(96,186,1080,207))]
    path = Path(session.env["XDG_STATE_HOME"]) / "little-screen-clip/organization.json"
    saved = lambda: json.loads(path.read_text())["session"] if path.exists() else {"marks":[],"enabled":False}
    colors = [(244,105,112),(229,188,55),(89,194,135)] if appearance=="dark" else [(174,36,46),(139,95,0),(23,121,73)]
    original = {name:(root/name).read_bytes() for name in ["alpha.txt","beta.txt","album/child.txt"]}
    text=session.observe("folder", ["alpha.txt","beta.txt","album","Session"])
    close_shelf(session,"folder",text)
    session.observe("folder-ready",["alpha.txt","beta.txt","Session"])
    open_session(session,"folder-ready","disabled-session",False)
    click_word(session,"disabled-session","Enable",minimum_y=170,maximum_y=400)
    wait_for(lambda:saved()["enabled"],"session enabled")
    session.key("Escape")
    for index,(name,color) in enumerate([("alpha.txt","Red"),("beta.txt","Yellow"),("album","Green")]):
        shot=f"before-mark-{index}"
        session.observe(shot,[name,"Session"])
        click_word(session,shot,name,minimum_y=230)
        selected=f"selected-{index}"
        session.observe(selected,[name,"Session"])
        menu=f"mark-menu-{index}"
        open_session(session,selected,menu)
        click_word(session,menu,color,minimum_y=170,maximum_y=550)
        wait_for(lambda:any(mark["target"]["path"]==str(root/name) and mark["color"]==color.lower() for mark in saved()["marks"]),f"{color} assignment saved")
    session.observe("three-marks",["alpha.txt","beta.txt","album"])
    assert all(line_count(session,"three-marks",color)>0 for color in colors),"all three semantic underlines must actually be painted"
    click_word(session,"three-marks","Columns",minimum_y=140,maximum_y=270)
    session.observe("columns-marks",["alpha.txt","beta.txt","album"])
    assert all(line_count(session,"columns-marks",color)>0 for color in colors)
    click_word(session,"columns-marks","album",minimum_y=230)
    session.key("Return")
    session.observe("marked-parent",["child.txt","album"])
    assert line_count(session,"marked-parent",colors[2])>0,"ancestor columns retain the same folder annotation"
    session.key("Up","alt")
    session.observe("returned-folder",["alpha.txt","beta.txt","album"])
    open_session(session,"returned-folder","definitions-menu")
    click_word(session,"definitions-menu","meanings",minimum_y=170,maximum_y=600)
    session.observe("definitions",["Save meanings","Session name","Green"])
    x,y=session.word("definitions","Green",required=True,minimum_y=300,last=True)
    session.click(x+90,y)
    session.key("a","ctrl")
    private.run_checked(["wtype","Reviewed"],env=session.env)
    click_word(session,"definitions","Save",minimum_y=350,maximum_y=730)
    wait_for(lambda:saved()["definitions"]["meanings"][2]=="Reviewed","custom meaning saved")
    session.key("Escape")
    session.observe("custom-meaning",["alpha.txt","beta.txt","Session"])
    open_session(session,"custom-meaning","clear-menu")
    click_word(session,"clear-menu","Clear",minimum_y=170,maximum_y=730)
    wait_for(lambda:not saved()["marks"] and len(saved()["previous"])==3,"clear retains one recoverable assignment set")
    session.observe("cleared",["alpha.txt","beta.txt"])
    assert not any(line_count(session,"cleared",color)>0 for color in colors)
    open_session(session,"cleared","restore-menu")
    click_word(session,"restore-menu","Restore",minimum_y=170,maximum_y=730)
    wait_for(lambda:len(saved()["marks"])==3,"restored session marks")
    session.observe("restored",["alpha.txt","beta.txt"])
    assert all(line_count(session,"restored",color)>0 for color in colors)
    session.close()
    session.restart(binary,root)
    text=session.observe("restarted",["alpha.txt","beta.txt","Session"])
    close_shelf(session,"restarted",text)
    session.observe("restarted-visible",["alpha.txt","beta.txt","Session"])
    assert len(saved()["marks"])==3
    assert saved()["definitions"]["meanings"][2]=="Reviewed"
    assert all(line_count(session,"restarted-visible",color)>0 for color in colors)
    open_session(session,"restarted-visible","invalid-definition-menu")
    click_word(session,"invalid-definition-menu","meanings",minimum_y=170,maximum_y=650)
    session.observe("invalid-definition-fields",["Save meanings","Reviewed"])
    x,y=session.word("invalid-definition-fields","Reviewed",minimum_y=300,last=True)
    session.click(x+20,y)
    session.key("a","ctrl")
    session.key("BackSpace")
    session.key("Escape")
    session.observe("invalid-draft-retained",["alpha.txt","beta.txt"])
    click_word(session,"invalid-draft-retained","alpha.txt",minimum_y=230)
    session.key("Return")
    session.observe("saved-text-studio",["Back to folder","alpha original"])
    session.key("q","ctrl")
    session.observe("definition-error-return",["Organization:","nonempty","Session"])
    assert saved()["definitions"]["meanings"][2]=="Reviewed","invalid draft must not replace stored definitions"
    text=open_session(session,"definition-error-return","discard-invalid-menu")
    if "save meanings" not in text.lower():
        click_word(session,"discard-invalid-menu","meanings",minimum_y=170,maximum_y=650)
    session.observe("discard-invalid-fields",["Discard definition edits","Save meanings"])
    click_word(session,"discard-invalid-fields","Discard",minimum_y=350,maximum_y=730)
    session.key("Escape")
    for name,bytes in original.items(): assert (root/name).read_bytes()==bytes


if __name__=="__main__":
    args=application_arguments(__doc__,Path("target/debug/clip-file-manager"))
    output,root=media_fixture_directory(args)
    (root/"alpha.txt").write_text("alpha original\n")
    (root/"beta.txt").write_text("beta original\n")
    (root/"album").mkdir()
    (root/"album/child.txt").write_text("child original\n")
    raise SystemExit(run_application_acceptance(args,output,root,
        lambda session,root:exercise(session,root,args.binary,args.appearance),
        "Session meanings, Red/Yellow/Green underlines in List and Columns, ancestor rows, clear/restore and restart without source changes"))
