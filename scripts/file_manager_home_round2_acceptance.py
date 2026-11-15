#!/usr/bin/env python3
"""Exercise the second Home review fixes in a private native desktop."""
import csv,io,json,uuid
from pathlib import Path
from collections import Counter
from PIL import Image,ImageOps
from file_manager_acceptance import application_arguments,media_fixture_directory,run_application_acceptance,wait_for
from file_manager_home_acceptance import click_word,drag
import ui_acceptance_capture as private


def verify_color_picker(session,find,section):
    session.observe("color-control",["Label"])
    with Image.open(session.output/"color-control.png").convert("RGB") as image:
        pixels=[(x,y) for y in range(145,205) for x in range(100,1100) if max(abs(a-b) for a,b in zip(image.getpixel((x,y)),(69,132,134)))<3]
    assert pixels,"actual section color swatch in inspector"
    cx=(min(x for x,y in pixels)+max(x for x,y in pixels))//2;cy=(min(y for x,y in pixels)+max(y for x,y in pixels))//2
    session.click(cx,cy);session.observe("color-popup",[])
    session.click(cx+100,cy+160)
    wait_for(lambda:find(section)["content"]["color"]!=[69,132,134],"first color change")
    first=find(section)["content"]["color"]
    session.observe("picker-retained",["Label"])
    session.click(cx+200,cy+250)
    wait_for(lambda:find(section)["content"]["color"]!=first,"second change through still-open picker")


def exercise(session,root,binary):
    session.observe("folder",["Home"]);session.close()
    folder=root/"PreviewRoom";folder.mkdir();(folder/"one.txt").write_text("Live preview")
    home=Path(session.env["XDG_STATE_HOME"])/"little-screen-clip/home.json";home.parent.mkdir(parents=True,exist_ok=True)
    pin,section,pane,shortcut=[str(uuid.uuid4()) for _ in range(4)]
    def card(identifier,rect,content):return {"id":identifier,"rect":rect,"content":content}
    cards=[card(section,[50,150,300,160],{"kind":"section","title":"Garden","color":[69,132,134]}),
        card(pane,[430,80,600,480],{"kind":"folder","pin":pin,"label":"Livefiles","options":{"presentation":"list","name":"","extensions":"","recent_minutes":0,"show_hidden":False}}),
        card(shortcut,[50,420,280,90],{"kind":"shortcut","pin":pin,"label":"Portal"})]
    regions=[{"id":str(uuid.uuid4()),"label":name,"points":[[900,550],[950,550],[950,600]],"action":{"kind":"open","pin":pin},"enabled":True} for name in ["Short","Much longer region name"]]
    home.write_text(json.dumps({"version":2,"pins":[{"id":pin,"path":str(folder),"folder":True}],"board":{"revision":0,"title":"Home","drawing":json.dumps({"type":"excalidraw","version":2,"elements":[],"appState":{},"files":{}}),"cards":cards,"bindings":[],"regions":regions,"view":{"center":[500,350],"scale":1}}}))
    read=lambda:json.loads(home.read_text())["board"]
    find=lambda identifier:next(c for c in read()["cards"] if c["id"]==identifier)
    session.restart(binary)
    session.observe("restored",["Garden","Livefiles","Portal"])
    initial=find(section)["rect"]
    point=session.word("restored","Garden",tree=True,minimum_y=180)
    drag(session,point,(point[0]+50,point[1]+30))
    session.observe("locked-drag",["Garden"])
    assert find(section)["rect"]==initial,"objects start locked against accidental dragging"
    assert "Label" not in (session.output/"locked-drag.txt").read_text(),"locked click must not select objects"
    click_word(session,"restored","Objects",minimum_y=90,maximum_y=145)
    session.observe("arrange",["Icons","Filter"])
    click_word(session,"arrange","Icons",minimum_y=180,maximum_y=400)
    wait_for(lambda:find(pane)["content"]["options"]["presentation"]=="icons","pane controls work with objects unlocked")
    session.observe("icons",["Filter"])
    click_word(session,"icons","Filter",minimum_y=180,maximum_y=400)
    session.observe("filter",["Name contains"])
    click_word(session,"filter","Objects",minimum_y=90,maximum_y=145,control_bounds=(485,102,560,136))
    session.observe("locked-again",["Garden"])
    click_word(session,"locked-again","Objects",minimum_y=90,maximum_y=145)
    session.observe("browse",["Garden"])
    click_word(session,"browse","Garden",minimum_y=180,maximum_y=350)
    session.observe("selected",["Label"])
    session.key("Delete")
    wait_for(lambda:all(c["id"]!=section for c in read()["cards"]),"Delete removes selected section with objects unlocked")
    session.key("z","ctrl")
    wait_for(lambda:any(c["id"]==section for c in read()["cards"]),"Ctrl+Z restores section")
    session.observe("undo",["Garden"])
    click_word(session,"undo","Garden",minimum_y=180,maximum_y=360)
    session.observe("properties",["Label"])
    x,y=session.word("properties","Label",tree=True)
    session.click(x+120,y+8);session.key("a","ctrl");private.run_checked(["wtype","Garden corner"],env=session.env)
    session.click(1100,700)
    wait_for(lambda:find(section)["content"]["title"]=="Garden corner","renaming restored section")
    session.observe("renamed",["Garden corner"])
    click_word(session,"renamed","Garden",minimum_y=180,maximum_y=360)
    verify_color_picker(session,find,section)
    session.key("Escape");session.click(1100,700)
    session.observe("before-regions",["Click regions"])
    click_word(session,"before-regions","Click",minimum_y=90,maximum_y=145)
    session.observe("left-regions",["Short","Much longer region name","Delete region"])
    with Image.open(session.output/"left-regions.png") as picture:
        ImageOps.autocontrast(picture.crop((105,315,480,510)).convert("L")).resize((1125,585)).save(session.output/"region-actions.png")
    data=private.run_checked(["tesseract",str(session.output/"region-actions.png"),"stdout","--psm","6","-c","tessedit_create_tsv=1","-c","tessedit_create_txt=0"],env=session.env)
    edits=[(int(w["left"])//3,int(w["top"])//3) for w in csv.DictReader(io.StringIO(data),delimiter="\t",quoting=csv.QUOTE_NONE) if w["text"].lower().startswith("edit")]
    assert len(edits)==2 and max(x for x,y in edits)-min(x for x,y in edits)<3 and all(x<500 for x,y in edits),edits
    session.close();session.restart(binary)
    session.observe("final-restart",["Garden corner","Livefiles","Portal"])
    click_word(session,"final-restart","Objects",minimum_y=90,maximum_y=145)
    session.observe("unlocked-restart",["Portal"])
    click_word(session,"unlocked-restart","Portal",minimum_y=350)
    session.key("Delete")
    wait_for(lambda:all(c["id"]!=shortcut for c in read()["cards"]),"restored folder shortcut can be selected and removed")
    session.key("z","ctrl")
    wait_for(lambda:any(c["id"]==shortcut for c in read()["cards"]),"folder shortcut restored by undo")
    session.observe("restored-shortcut",["Portal"])
    point=session.word("restored-shortcut","Portal",tree=True,minimum_y=350)
    before=find(shortcut)["rect"]
    drag(session,point,(point[0]+45,point[1]+25))
    wait_for(lambda:find(shortcut)["rect"]!=before,"restored folder shortcut can move with objects unlocked")
    session.scroll(640,700,1000)
    wait_for(lambda:read()["view"]["scale"]<0.3,"zoom-out has breathing room")
    session.observe("zoomed-out",[])
    with Image.open(session.output/"zoomed-out.png").convert("RGB") as picture:
        backgrounds=[]
        for bounds in [(105,680,135,720),(625,680,655,720),(1145,680,1175,720)]:
            crop=picture.crop(bounds)
            backgrounds.append(Counter(getattr(crop,"get_flattened_data",crop.getdata)()).most_common(1)[0][0])
    assert backgrounds[0]==backgrounds[1]==backgrounds[2],"canvas must fill both sides without gutters"
    assert (folder/"one.txt").read_text()=="Live preview"


if __name__=="__main__":
    args=application_arguments(__doc__,Path("target/debug/clip-file-manager"));output,root=media_fixture_directory(args)
    raise SystemExit(run_application_acceptance(args,output,root,lambda session,root:exercise(session,root,args.binary),
        "Restored object editing, Delete/Undo, folder controls with objects unlocked, persistent color popup and aligned left regions"))
