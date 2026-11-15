#!/usr/bin/env python3
"""Exercise a live Home folder pane, shared previews and local filters natively."""
import json
import math
import os
from pathlib import Path
import struct
import time
import wave
from PIL import Image,ImageDraw
from file_manager_acceptance import application_arguments,media_fixture_directory,run_application_acceptance,wait_for
from file_manager_home_acceptance import click_word,enter_home,click_cropped_word
import ui_acceptance_capture as private


def fixtures(root):
    picture=Image.new("RGB",(320,180),(240,120,30))
    ImageDraw.Draw(picture).rectangle((160,0,319,179),fill=(25,180,110))
    picture.save(root/"photo.png")
    with wave.open(str(root/"tone.wav"),"wb") as output:
        output.setparams((1,2,16000,0,"NONE","not compressed"))
        output.writeframes(b"".join(struct.pack("<h",int(15000*math.sin(2*math.pi*440*i/16000))) for i in range(48000)))
    (root/"old.wav").write_bytes((root/"tone.wav").read_bytes())
    old=time.time()-7200
    os.utime(root/"old.wav",(old,old))
    (root/".hide").write_text("version=1\npatterns=['*.secret']\n")
    (root/"invisible.secret").write_text("hidden by existing folder policy")


def pane_control(session,shot,label):
    try:
        click_word(session,shot,label,minimum_y=145)
    except RuntimeError:
        # Scope fallback to the actual pane controls observed in this private
        # 1280x800 fixture. OCR can omit labels against the tinted panel.
        bounds={"Filter":(650,315,715,358),"Icons":(584,315,647,358),"List":(528,315,582,358)}[label]
        if not click_cropped_word(session,shot,label,bounds,145,400):
            session.click((bounds[0]+bounds[2])//2,(bounds[1]+bounds[3])//2)


def exercise(session,root,binary):
    state=Path(session.env["XDG_STATE_HOME"])/"little-screen-clip/home.json"
    read=lambda:json.loads(state.read_text())
    pane=lambda:next(c for c in read()["board"]["cards"] if c["content"]["kind"]=="folder")
    session.observe("folder",["Home"])
    enter_home(session,"folder")
    session.observe("home",["Make this place yours"])
    click_word(session,"home","Add",minimum_y=90,maximum_y=145)
    session.observe("add",["Live folder pane"])
    click_word(session,"add","Live",minimum_y=135,maximum_y=400)
    session.observe("folder-sources",["Current folder"])
    click_word(session,"folder-sources","Current",minimum_y=135,maximum_y=300)
    wait_for(lambda:state.exists() and any(c["content"]["kind"]=="folder" for c in read()["board"]["cards"]),"folder pane persisted")
    session.observe("pane",["matching items","Open folder"])
    # Show the photo alone using the actual inline controls, then verify painted
    # source colors, not merely the presence of its filename.
    pane_control(session,"pane","Filter")
    session.observe("filter",["Name contains","Extensions"])
    x,y=session.word("filter","Extensions",tree=True,required=False) or session.word("filter","Extensions")
    session.click(x+115,y)
    private.run_checked(["wtype","png"],env=session.env)
    wait_for(lambda:pane()["content"]["options"]["extensions"]=="png","pane-local extension filter saved")
    session.observe("photo-filtered",["Extensions"])
    pane_control(session,"photo-filtered","Filter")
    session.observe("photo-only",["photo.png","1 matching"])
    def photo_pixels():
        session.capture_text("painted-photo",session.output/"painted-photo.png",[])
        with Image.open(session.output/"painted-photo.png").convert("RGB") as picture:
            return sum(pixel==(240,120,30) for pixel in getattr(picture,"get_flattened_data",picture.getdata)())>100
    wait_for(photo_pixels,"real shared image strip rendered")
    # Change that same field to audio and apply a relative-age preset.
    pane_control(session,"photo-only","Filter")
    session.observe("photo-filter-open",["Extensions"])
    x,y=session.word("photo-filter-open","Extensions",tree=True,required=False) or session.word("photo-filter-open","Extensions")
    session.click(x+115,y);session.key("a","ctrl")
    private.run_checked(["wtype","wav"],env=session.env)
    if not click_cropped_word(session,"photo-filter-open","Last",(650,400,746,444),145,500):
        x,y=session.word("photo-filter-open","Modified")
        session.click(x+279,y+8)
    wait_for(lambda:pane()["content"]["options"]["recent_minutes"]==60,"last-hour filter saved")
    session.observe("audio-filtered",["Extensions"])
    pane_control(session,"audio-filtered","Filter")
    session.observe("recent-audio",["tone.wav","1 matching","3 s"])
    assert "old.wav" not in (session.output/"recent-audio.txt").read_text()
    def waveform_pixels():
        session.capture_text("painted-waveform",session.output/"painted-waveform.png",[])
        with Image.open(session.output/"painted-waveform.png").convert("RGB") as picture:
            return sum(g>r+10 and g>b+30 and b<60 for r,g,b in getattr(picture,"get_flattened_data",picture.getdata)())>100
    wait_for(waveform_pixels,"shared frequency-colored audio waveform rendered")
    pane_control(session,"recent-audio","Icons")
    wait_for(lambda:pane()["content"]["options"]["presentation"]=="icons","icon grid selected")
    session.observe("icon-grid",["tone.wav"])
    (root/"recent.wav").write_bytes((root/"tone.wav").read_bytes())
    session.observe("external-update",["recent.wav","tone.wav","2 matching"])
    saved=pane()["content"]["options"]
    assert saved["extensions"]=="wav" and saved["recent_minutes"]==60
    assert (root/".hide").read_text()=="version=1\npatterns=['*.secret']\n"
    assert not (root/".show").exists()
    session.close();session.restart(binary)
    session.observe("restarted",["recent.wav","tone.wav","2 matching"])
    assert pane()["content"]["options"]==saved
    # Add another instance of the same folder with independent filters, then
    # move its header directly so both shared media renderers remain visible.
    pane_control(session,"restarted","List")
    wait_for(lambda:pane()["content"]["options"]["presentation"]=="list","first pane list restored")
    session.observe("first-list",["recent.wav"])
    click_word(session,"first-list","Add",minimum_y=90,maximum_y=145)
    session.observe("add-second",["Live folder pane"])
    click_word(session,"add-second","Live",minimum_y=135,maximum_y=400)
    session.observe("saved-folder-sources",["Current folder","fixture"])
    click_word(session,"saved-folder-sources","fixture",minimum_y=135,maximum_y=300)
    wait_for(lambda:len(read()["board"]["cards"])==2,"independent second pane added")
    session.observe("second-pane",["matching items"])
    pane_control(session,"second-pane","Filter")
    session.observe("second-filter",["Extensions"])
    x,y=session.word("second-filter","Extensions",tree=True,required=False) or session.word("second-filter","Extensions")
    session.click(x+115,y);private.run_checked(["wtype","png"],env=session.env)
    wait_for(lambda:read()["board"]["cards"][1]["content"]["options"]["extensions"]=="png","second pane independently filtered")
    session.observe("second-filtered",["Extensions"])
    pane_control(session,"second-filtered","Filter")
    session.observe("second-photo",["photo.png"])
    click_word(session,"second-photo","Objects",minimum_y=90,maximum_y=145)
    session.observe("unlocked-pane",["photo.png"])
    before=read()["board"]["cards"][1]["rect"][:]
    private.run_checked([str(session.pointer),"drag",*map(str,session.logical_size),"640","298","1000","298"],env=session.env)
    wait_for(lambda:read()["board"]["cards"][1]["rect"][0]>before[0]+100,"live pane moved with objects unlocked")
    session.observe("two-live-panes",["recent.wav","photo.png"])
    wait_for(photo_pixels,"photo preview survives alongside the other pane")
    wait_for(waveform_pixels,"waveform preview survives alongside the other pane")
    cards=read()["board"]["cards"]
    assert cards[0]["content"]["pin"]==cards[1]["content"]["pin"]
    assert cards[0]["content"]["options"]["extensions"]=="wav"
    assert cards[1]["content"]["options"]["extensions"]=="png"


if __name__=="__main__":
    args=application_arguments(__doc__,Path("target/debug/clip-file-manager"))
    output,root=media_fixture_directory(args);fixtures(root)
    raise SystemExit(run_application_acceptance(args,output,root,
        lambda session,root:exercise(session,root,args.binary),
        "Live shared image/audio rows, Home-only filters, relative age, icon grid, external refresh and restart"))
