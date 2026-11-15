#!/usr/bin/env python3
"""Verify persisted measured-pressure ink with the actual shared OpenGL renderer.

This tests paint/history/restart and mouse isolation, not physical tablet hardware.
"""
import json
import uuid
from pathlib import Path
from PIL import Image
from file_manager_acceptance import application_arguments,media_fixture_directory,run_application_acceptance,wait_for
from file_manager_home_acceptance import click_word,drag


def exercise(session,root,binary):
    session.observe("folder",["Home"]);session.close()
    home=Path(session.env["XDG_STATE_HOME"])/"little-screen-clip/home.json"
    home.parent.mkdir(parents=True,exist_ok=True)
    drawing=json.dumps({"type":"excalidraw","version":2,"elements":[],"appState":{},"files":{}})
    strokes=[]
    for y,pressures in [(150,[0.1]*5),(250,[0.9]*5),(350,[0.1,0.3,0.5,0.7,0.9])]:
        strokes.append({"id":str(uuid.uuid4()),"origin":[200,y],"points":[[i*80,0,p] for i,p in enumerate(pressures)],"width":2.,"color":"theme","marker":False})
    home.write_text(json.dumps({"version":2,"pins":[],"board":{"revision":0,"title":"Pressure check","drawing":drawing,
        "cards":[],"bindings":[],"ink":strokes,"view":{"center":[500,350],"scale":1.}}}))
    read=lambda:json.loads(home.read_text())["board"]
    session.restart(binary)
    session.observe("ink",["Pressure check","Ink"])
    target=lambda color: min(color)>210 if session.env.get("ACCEPTANCE_INK_DARK")=="1" else max(color)<60
    # The private output's observed canvas is (98,145)..(1182,745); the persisted
    # camera maps (200,150) to (340,245). Sample interior columns, away from caps.
    def painted():
        session.capture_text("native",session.output/"native.png",[])
        with Image.open(session.output/"native.png").convert("RGB") as screen:
            thickness=lambda x,y:sum(target(screen.getpixel((x,yy))) for yy in range(y-25,y+26))
            soft,hard=thickness(480,245),thickness(480,345)
            low,high=thickness(380,445),thickness(590,445)
        return soft>=1 and hard>soft*1.5 and high>low
    wait_for(painted,"native pressure-dependent stroke thickness")
    original=read().get("ink",[])
    drag(session,(450,550),(700,570))
    session.observe("mouse-isolation",["Pressure check"])
    assert read().get("ink",[])==original,"ordinary mouse dragging must never become pen drawing"
    click_word(session,"mouse-isolation","Ink",minimum_y=90,maximum_y=145)
    session.observe("brushes",["Marker","Connect a pressure-sensitive"])
    click_word(session,"brushes","Clear",minimum_y=145)
    wait_for(lambda:not read().get("ink",[]),"explicit clear ink saved")
    session.observe("cleared",["Pressure check"])
    # With no remaining ink and no physical pen the compact Ink menu disappears;
    # Undo returns to the standard, title-dependent toolbar geometry.
    click_word(session,"cleared","Undo",minimum_y=90,maximum_y=145,control_bounds=(617,103,679,134))
    wait_for(lambda:read().get("ink",[])==original,"ink restored through ordinary layout history")
    session.observe("restored",["Pressure check","Ink"])
    session.close();session.restart(binary)
    session.observe("restart",["Pressure check","Ink"])
    assert read().get("ink",[])==original and read()["drawing"]==drawing


if __name__=="__main__":
    args=application_arguments(__doc__,Path("target/debug/clip-file-manager"))
    output,root=media_fixture_directory(args)
    def prepare(env,children):env["ACCEPTANCE_INK_DARK"]="1" if args.appearance=="dark" else "0"
    raise SystemExit(run_application_acceptance(args,output,root,
        lambda session,root:exercise(session,root,args.binary),
        "Native pressure geometry, theme visibility, mouse isolation, brush controls, clear/undo and restart; no physical tablet claim",prepare_environment=prepare))
