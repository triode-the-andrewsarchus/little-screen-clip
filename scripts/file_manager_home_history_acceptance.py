#!/usr/bin/env python3
"""Exercise virtual Home history, folder return and no-argument startup."""
from pathlib import Path
from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance
from file_manager_home_acceptance import click_word, enter_home
from file_manager_session_acceptance import close_shelf


def exercise(session,root,binary):
    session.ocr_regions=[("home-history-controls-ocr.png",(185,98,780,138))]
    text=session.observe("folder",["alpha.txt","beta.txt","Home"])
    close_shelf(session,"folder",text)
    session.observe("folder-ready",["alpha.txt","beta.txt","Home"])
    click_word(session,"folder-ready","alpha.txt",minimum_y=230)
    session.observe("selected",["alpha.txt","Home"])
    enter_home(session,"selected")
    session.observe("home",["Back to folder","Objects","Make this place yours"])
    session.key("Left","alt")
    session.observe("history-back-folder",["alpha.txt","beta.txt","Rules:"])
    session.key("Right","alt")
    session.observe("history-forward-home",["Back to folder","Objects","Make this place yours"])
    session.key("Escape")
    session.observe("escape-folder",["alpha.txt","beta.txt","Rules:"])
    session.key("Right","alt")
    session.observe("forward-after-escape",["Back to folder","Objects","Make this place yours"])
    click_word(session,"forward-after-escape","Back",minimum_y=85,maximum_y=150)
    session.observe("button-folder",["alpha.txt","beta.txt","Rules:"])
    session.close();session.restart(binary)
    text=session.observe("no-argument-home",["Back to folder","Objects","Make this place yours"])
    assert "Rules:" not in text,"no-argument startup should show the personal Home"
    session.key("Escape")
    session.observe("startup-real-folder",[str(Path(session.env["HOME"])),"Empty folder"])
    session.key("Left","alt")
    session.observe("startup-history-home",["Back to folder","Objects","Make this place yours"])
    assert (root/"alpha.txt").read_text()=="alpha original\n"
    assert (root/"beta.txt").read_text()=="beta original\n"


if __name__=="__main__":
    args=application_arguments(__doc__,Path("target/debug/clip-file-manager"))
    output,root=media_fixture_directory(args)
    for name in ["alpha","beta"]:(root/(name+".txt")).write_text(name+" original\n")
    raise SystemExit(run_application_acceptance(args,output,root,
        lambda session,root:exercise(session,root,args.binary),
        "Home Back/Forward/Escape/button history, no-argument startup and explicit real folder context"))
